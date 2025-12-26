use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token_interface::{self, Mint, MintTo, Burn, TokenAccount, TokenInterface},
};

declare_id!("GUSD1111111111111111111111111111111111111111");

// ============================================================================
// CONSTANTS
// ============================================================================

/// Collateral ratio required to mint (150% = 15000 basis points)
pub const MIN_COLLATERAL_RATIO_BPS: u64 = 15000;

/// Collateral ratio below which liquidation is allowed (120% = 12000 basis points)
pub const LIQUIDATION_THRESHOLD_BPS: u64 = 12000;

/// Liquidation penalty (10% = 1000 basis points)
pub const LIQUIDATION_PENALTY_BPS: u64 = 1000;

/// Basis points denominator
pub const BPS_DENOMINATOR: u64 = 10000;

/// GUSD decimals (6, like USDC)
pub const GUSD_DECIMALS: u8 = 6;

/// GOR decimals (9, like SOL)
pub const GOR_DECIMALS: u8 = 9;

/// Maximum price change per update (20% = 2000 BPS) [MEDIUM-1]
pub const MAX_PRICE_CHANGE_BPS: u64 = 2000;

/// Minimum seconds between admin price updates (MVP safety)
pub const MIN_PRICE_UPDATE_INTERVAL_SECS: i64 = 1;

// ============================================================================
// PROGRAM
// ============================================================================

#[program]
pub mod gusd_stablecoin {
    use super::*;

    /// Initialize the GUSD protocol
    /// Creates the global state and GUSD mint
    /// 
    /// # Arguments
    /// * `initial_gor_price_usd` - GOR price in USD with 6 decimals
    ///   Examples: 
    ///   - 4776 = $0.004776 (current sGOR price)
    ///   - 1_000_000 = $1.00
    ///   - 10_000 = $0.01
    pub fn initialize(ctx: Context<Initialize>, initial_gor_price_usd: u64) -> Result<()> {
        // [LOW-1] Validate initial price
        require!(initial_gor_price_usd > 0, GusdError::InvalidPrice);

        let protocol = &mut ctx.accounts.protocol_state;
        
        protocol.admin = ctx.accounts.admin.key();
        protocol.gusd_mint = ctx.accounts.gusd_mint.key();
        protocol.gor_price_usd = initial_gor_price_usd; // Price in USD with 6 decimals
        protocol.total_collateral = 0;
        protocol.total_debt = 0;
        protocol.bump = ctx.bumps.protocol_state;
        protocol.mint_bump = ctx.bumps.gusd_mint;
        protocol.is_paused = false; // [MEDIUM-2] Initialize pause state
        protocol.last_price_update_ts = Clock::get()?.unix_timestamp;

        msg!("GUSD Protocol initialized!");
        msg!("Initial GOR price: ${}", initial_gor_price_usd as f64 / 1_000_000.0);
        
        Ok(())
    }

    /// Update the GOR/USD price (admin only for MVP)
    /// In production, this would use an oracle like Pyth
    /// [MEDIUM-1] Now includes price change limits
    pub fn update_price(ctx: Context<UpdatePrice>, new_gor_price_usd: u64) -> Result<()> {
        require!(new_gor_price_usd > 0, GusdError::InvalidPrice);

        let protocol = &mut ctx.accounts.protocol_state;
        let old_price = protocol.gor_price_usd;

        // Enforce a minimum update interval (helps mitigate admin compromise / fat-finger risk)
        let now = Clock::get()?.unix_timestamp;
        let elapsed = now.saturating_sub(protocol.last_price_update_ts);
        require!(
            elapsed >= MIN_PRICE_UPDATE_INTERVAL_SECS,
            GusdError::PriceUpdateTooFrequent
        );

        // [MEDIUM-1] Calculate absolute price change
        let price_change = if new_gor_price_usd > old_price {
            new_gor_price_usd.saturating_sub(old_price)
        } else {
            old_price.saturating_sub(new_gor_price_usd)
        };

        // [MEDIUM-1] Check change is within 20% limit (ceiling division so small prices still move)
        let max_change_u128 = (old_price as u128)
            .checked_mul(MAX_PRICE_CHANGE_BPS as u128)
            .ok_or(GusdError::MathOverflow)?
            .checked_add((BPS_DENOMINATOR - 1) as u128)
            .ok_or(GusdError::MathOverflow)?
            .checked_div(BPS_DENOMINATOR as u128)
            .ok_or(GusdError::MathOverflow)?
            .max(1);

        require!(max_change_u128 <= u64::MAX as u128, GusdError::MathOverflow);
        let max_change = max_change_u128 as u64;

        require!(price_change <= max_change, GusdError::PriceChangeExceedsLimit);

        protocol.gor_price_usd = new_gor_price_usd;
        protocol.last_price_update_ts = now;

        msg!("GOR price updated: {} -> {}", old_price, new_gor_price_usd);

        // [MEDIUM-3] Emit event
        emit!(PriceUpdated {
            old_price,
            new_price: new_gor_price_usd,
        });

        Ok(())
    }

    /// [MEDIUM-2] Pause protocol (admin only)
    pub fn pause_protocol(ctx: Context<UpdatePrice>) -> Result<()> {
        ctx.accounts.protocol_state.is_paused = true;
        msg!("Protocol paused");
        Ok(())
    }

    /// [MEDIUM-2] Unpause protocol (admin only)
    pub fn unpause_protocol(ctx: Context<UpdatePrice>) -> Result<()> {
        ctx.accounts.protocol_state.is_paused = false;
        msg!("Protocol unpaused");
        Ok(())
    }

    /// [LOW-2] Transfer admin role to a new address
    pub fn transfer_admin(ctx: Context<TransferAdmin>, new_admin: Pubkey) -> Result<()> {
        require!(new_admin != Pubkey::default(), GusdError::InvalidAmount);
        
        let protocol = &mut ctx.accounts.protocol_state;
        let old_admin = protocol.admin;
        protocol.admin = new_admin;
        
        msg!("Admin transferred from {} to {}", old_admin, new_admin);
        
        Ok(())
    }

    /// Create a new vault for a user
    /// [CRITICAL-4] Now initializes vault_collateral PDA
    pub fn create_vault(ctx: Context<CreateVault>) -> Result<()> {
        let vault = &mut ctx.accounts.vault;
        
        vault.owner = ctx.accounts.owner.key();
        vault.collateral_amount = 0;
        vault.debt_amount = 0;
        vault.bump = ctx.bumps.vault;
        vault.collateral_bump = ctx.bumps.vault_collateral; // [CRITICAL-4] Store collateral bump

        msg!("Vault created for user: {}", ctx.accounts.owner.key());

        // [MEDIUM-3] Emit event
        emit!(VaultCreated {
            owner: ctx.accounts.owner.key(),
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Deposit GOR collateral into a vault
    pub fn deposit_collateral(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
        require!(amount > 0, GusdError::InvalidAmount);

        // Transfer GOR from user to vault's collateral account
        let cpi_accounts = anchor_lang::system_program::Transfer {
            from: ctx.accounts.owner.to_account_info(),
            to: ctx.accounts.vault_collateral.to_account_info(),
        };
        let cpi_program = ctx.accounts.system_program.to_account_info();
        anchor_lang::system_program::transfer(
            CpiContext::new(cpi_program, cpi_accounts),
            amount,
        )?;

        // Update vault state
        let vault = &mut ctx.accounts.vault;
        vault.collateral_amount = vault.collateral_amount.checked_add(amount)
            .ok_or(GusdError::MathOverflow)?;

        // Update protocol totals
        let protocol = &mut ctx.accounts.protocol_state;
        protocol.total_collateral = protocol.total_collateral.checked_add(amount)
            .ok_or(GusdError::MathOverflow)?;

        msg!("Deposited {} GOR. Total collateral: {}", amount, vault.collateral_amount);

        // [MEDIUM-3] Emit event
        emit!(CollateralDeposited {
            owner: ctx.accounts.owner.key(),
            amount,
            total_collateral: vault.collateral_amount,
        });
        
        Ok(())
    }

    /// Mint GUSD against deposited collateral
    pub fn mint_gusd(ctx: Context<MintGusd>, amount: u64) -> Result<()> {
        // [MEDIUM-2] Check pause state
        require!(!ctx.accounts.protocol_state.is_paused, GusdError::ProtocolPaused);
        require!(amount > 0, GusdError::InvalidAmount);

        let vault = &mut ctx.accounts.vault;
        let protocol = &ctx.accounts.protocol_state;

        // Calculate new debt
        let new_debt = vault.debt_amount.checked_add(amount)
            .ok_or(GusdError::MathOverflow)?;

        // Check collateral ratio after minting
        let collateral_value_usd = calculate_usd_value(
            vault.collateral_amount,
            protocol.gor_price_usd,
            GOR_DECIMALS,
        )?;

        let required_collateral = new_debt
            .checked_mul(MIN_COLLATERAL_RATIO_BPS)
            .ok_or(GusdError::MathOverflow)?
            .checked_div(BPS_DENOMINATOR)
            .ok_or(GusdError::MathOverflow)?;

        require!(
            collateral_value_usd >= required_collateral,
            GusdError::InsufficientCollateral
        );

        // Mint GUSD to user
        let seeds = &[
            b"protocol".as_ref(),
            &[protocol.bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = MintTo {
            mint: ctx.accounts.gusd_mint.to_account_info(),
            to: ctx.accounts.user_gusd_account.to_account_info(),
            authority: ctx.accounts.protocol_state.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        
        token_interface::mint_to(
            CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds),
            amount,
        )?;

        // Update vault debt
        vault.debt_amount = new_debt;

        // [HIGH-3] Fixed: Don't clone protocol_state
        let protocol = &mut ctx.accounts.protocol_state;
        protocol.total_debt = protocol.total_debt
            .checked_add(amount)
            .ok_or(GusdError::MathOverflow)?;

        // Calculate collateral ratio for event
        let collateral_ratio_bps = collateral_value_usd
            .checked_mul(BPS_DENOMINATOR)
            .ok_or(GusdError::MathOverflow)?
            .checked_div(new_debt)
            .ok_or(GusdError::MathOverflow)?;

        msg!("Minted {} GUSD. Total debt: {}", amount, vault.debt_amount);
        msg!("Collateral ratio: {}%", collateral_ratio_bps as f64 / 100.0);

        // [MEDIUM-3] Emit event
        emit!(GusdMinted {
            owner: ctx.accounts.owner.key(),
            amount,
            total_debt: vault.debt_amount,
            collateral_ratio_bps,
        });
        
        Ok(())
    }

    /// Repay GUSD debt (burns GUSD)
    pub fn repay_gusd(ctx: Context<RepayGusd>, amount: u64) -> Result<()> {
        require!(amount > 0, GusdError::InvalidAmount);
        
        let vault = &mut ctx.accounts.vault;
        
        // Can't repay more than owed
        let repay_amount = amount.min(vault.debt_amount);

        // Burn GUSD from user
        let cpi_accounts = Burn {
            mint: ctx.accounts.gusd_mint.to_account_info(),
            from: ctx.accounts.user_gusd_account.to_account_info(),
            authority: ctx.accounts.owner.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        
        token_interface::burn(
            CpiContext::new(cpi_program, cpi_accounts),
            repay_amount,
        )?;

        // Update vault debt
        vault.debt_amount = vault.debt_amount.checked_sub(repay_amount)
            .ok_or(GusdError::MathOverflow)?;

        // Update protocol totals
        let protocol = &mut ctx.accounts.protocol_state;
        protocol.total_debt = protocol.total_debt.checked_sub(repay_amount)
            .ok_or(GusdError::MathOverflow)?;

        msg!("Repaid {} GUSD. Remaining debt: {}", repay_amount, vault.debt_amount);

        // [MEDIUM-3] Emit event
        emit!(GusdRepaid {
            owner: ctx.accounts.owner.key(),
            amount: repay_amount,
            remaining_debt: vault.debt_amount,
        });
        
        Ok(())
    }

    /// Withdraw collateral (if ratio remains healthy)
    /// [CRITICAL-1] Fixed: Uses PDA-signed system transfer
    pub fn withdraw_collateral(ctx: Context<WithdrawCollateral>, amount: u64) -> Result<()> {
        // [MEDIUM-2] Check pause state
        require!(!ctx.accounts.protocol_state.is_paused, GusdError::ProtocolPaused);
        require!(amount > 0, GusdError::InvalidAmount);

        let vault = &mut ctx.accounts.vault;
        let protocol = &ctx.accounts.protocol_state;

        require!(
            amount <= vault.collateral_amount,
            GusdError::InsufficientCollateral
        );

        // Calculate remaining collateral after withdrawal
        let remaining_collateral = vault.collateral_amount.checked_sub(amount)
            .ok_or(GusdError::MathOverflow)?;

        // If there's debt, check that ratio stays healthy
        if vault.debt_amount > 0 {
            let remaining_value_usd = calculate_usd_value(
                remaining_collateral,
                protocol.gor_price_usd,
                GOR_DECIMALS,
            )?;

            let required_collateral = vault.debt_amount
                .checked_mul(MIN_COLLATERAL_RATIO_BPS)
                .ok_or(GusdError::MathOverflow)?
                .checked_div(BPS_DENOMINATOR)
                .ok_or(GusdError::MathOverflow)?;

            require!(
                remaining_value_usd >= required_collateral,
                GusdError::WouldUndercollateralize
            );
        }

        // [CRITICAL-1] Fixed: Use PDA-signed transfer instead of direct lamport manipulation
        let owner_key = ctx.accounts.owner.key();
        let seeds = &[
            b"vault_collateral".as_ref(),
            owner_key.as_ref(),
            &[ctx.bumps.vault_collateral],
        ];
        let signer_seeds = &[&seeds[..]];

        anchor_lang::system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.vault_collateral.to_account_info(),
                    to: ctx.accounts.owner.to_account_info(),
                },
                signer_seeds,
            ),
            amount,
        )?;

        // Update vault state
        vault.collateral_amount = remaining_collateral;

        // Update protocol totals
        let protocol = &mut ctx.accounts.protocol_state;
        protocol.total_collateral = protocol.total_collateral.checked_sub(amount)
            .ok_or(GusdError::MathOverflow)?;

        msg!("Withdrew {} GOR. Remaining collateral: {}", amount, vault.collateral_amount);

        // [MEDIUM-3] Emit event
        emit!(CollateralWithdrawn {
            owner: ctx.accounts.owner.key(),
            amount,
            remaining_collateral: vault.collateral_amount,
        });
        
        Ok(())
    }

    /// Close an empty vault (debt == 0 and tracked collateral == 0)
    /// Transfers any remaining lamports in the collateral PDA (e.g., rent) back to the owner.
    pub fn close_vault(ctx: Context<CloseVault>) -> Result<()> {
        require!(ctx.accounts.vault.debt_amount == 0, GusdError::VaultNotEmpty);
        require!(ctx.accounts.vault.collateral_amount == 0, GusdError::VaultNotEmpty);

        // Drain any remaining lamports (rent, etc.) from the collateral PDA back to the owner.
        let vault_owner_key = ctx.accounts.owner.key();
        let vault_collateral_bump = ctx.accounts.vault.collateral_bump;

        let balance = **ctx.accounts.vault_collateral.lamports.borrow();
        if balance > 0 {
            let seeds = &[
                b"vault_collateral".as_ref(),
                vault_owner_key.as_ref(),
                &[vault_collateral_bump],
            ];
            let signer_seeds = &[&seeds[..]];

            anchor_lang::system_program::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.system_program.to_account_info(),
                    anchor_lang::system_program::Transfer {
                        from: ctx.accounts.vault_collateral.to_account_info(),
                        to: ctx.accounts.owner.to_account_info(),
                    },
                    signer_seeds,
                ),
                balance,
            )?;
        }

        msg!("Vault closed: {}", vault_owner_key);
        Ok(())
    }

    /// Liquidate an undercollateralized vault
    /// [CRITICAL-2] Fixed: Uses PDA-signed system transfer
    /// [CRITICAL-3] Fixed: Correct liquidation math
    pub fn liquidate(ctx: Context<Liquidate>) -> Result<()> {
        // [MEDIUM-2] Check pause state
        require!(!ctx.accounts.protocol_state.is_paused, GusdError::ProtocolPaused);

        // Snapshot values we need before taking mutable borrows
        let vault_owner_key = ctx.accounts.vault_owner.key();
        let price = ctx.accounts.protocol_state.gor_price_usd;

        let vault_collateral_amount = ctx.accounts.vault.collateral_amount;
        let vault_debt_amount = ctx.accounts.vault.debt_amount;
        let vault_collateral_bump = ctx.accounts.vault.collateral_bump;

        require!(vault_debt_amount > 0, GusdError::NoDebtToLiquidate);

        // Check if vault is undercollateralized
        let collateral_value_usd = calculate_usd_value(
            vault_collateral_amount,
            price,
            GOR_DECIMALS,
        )?;

        let collateral_ratio_bps = (collateral_value_usd as u128)
            .checked_mul(BPS_DENOMINATOR as u128)
            .ok_or(GusdError::MathOverflow)?
            .checked_div(vault_debt_amount as u128)
            .ok_or(GusdError::MathOverflow)?;

        require!(
            collateral_ratio_bps < LIQUIDATION_THRESHOLD_BPS as u128,
            GusdError::VaultNotLiquidatable
        );

        // Determine the maximum profitable repay amount given available collateral.
        // We only allow liquidations where: collateral_seized >= repay_amount * (1 + penalty)
        let bonus_denominator = (BPS_DENOMINATOR + LIQUIDATION_PENALTY_BPS) as u128;

        let max_repay_u128 = (collateral_value_usd as u128)
            .checked_mul(BPS_DENOMINATOR as u128)
            .ok_or(GusdError::MathOverflow)?
            .checked_div(bonus_denominator)
            .ok_or(GusdError::MathOverflow)?;

        let repay_u128 = (vault_debt_amount as u128).min(max_repay_u128);
        require!(repay_u128 > 0, GusdError::LiquidationNotProfitable);
        require!(repay_u128 <= u64::MAX as u128, GusdError::MathOverflow);
        let repay_amount = repay_u128 as u64;

        // Burn GUSD from liquidator
        let cpi_accounts = Burn {
            mint: ctx.accounts.gusd_mint.to_account_info(),
            from: ctx.accounts.liquidator_gusd_account.to_account_info(),
            authority: ctx.accounts.liquidator.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();

        token_interface::burn(
            CpiContext::new(cpi_program, cpi_accounts),
            repay_amount,
        )?;

        // Calculate USD value with liquidation bonus
        let repay_with_bonus_u128 = (repay_amount as u128)
            .checked_mul(bonus_denominator)
            .ok_or(GusdError::MathOverflow)?
            .checked_div(BPS_DENOMINATOR as u128)
            .ok_or(GusdError::MathOverflow)?;

        // Convert USD (6 decimals) to GOR lamports (9 decimals)
        let collateral_to_liquidator_u128 = repay_with_bonus_u128
            .checked_mul(10u128.pow(GOR_DECIMALS as u32))
            .ok_or(GusdError::MathOverflow)?
            .checked_div(price as u128)
            .ok_or(GusdError::MathOverflow)?;

        require!(collateral_to_liquidator_u128 <= u64::MAX as u128, GusdError::MathOverflow);
        let collateral_to_liquidator = collateral_to_liquidator_u128 as u64;

        // Final sanity check: do not seize more than tracked collateral
        require!(
            collateral_to_liquidator <= vault_collateral_amount,
            GusdError::MathOverflow
        );

        // Transfer collateral to liquidator (PDA signed)
        let seeds = &[
            b"vault_collateral".as_ref(),
            vault_owner_key.as_ref(),
            &[vault_collateral_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        anchor_lang::system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.vault_collateral.to_account_info(),
                    to: ctx.accounts.liquidator.to_account_info(),
                },
                signer_seeds,
            ),
            collateral_to_liquidator,
        )?;

        // Update protocol totals
        let protocol = &mut ctx.accounts.protocol_state;
        protocol.total_collateral = protocol.total_collateral
            .checked_sub(collateral_to_liquidator)
            .ok_or(GusdError::MathOverflow)?;
        protocol.total_debt = protocol.total_debt
            .checked_sub(repay_amount)
            .ok_or(GusdError::MathOverflow)?;

        // Update vault
        let vault = &mut ctx.accounts.vault;
        vault.collateral_amount = vault.collateral_amount
            .checked_sub(collateral_to_liquidator)
            .ok_or(GusdError::MathOverflow)?;
        vault.debt_amount = vault.debt_amount
            .checked_sub(repay_amount)
            .ok_or(GusdError::MathOverflow)?;

        msg!(
            "Liquidation: repaid {} GUSD, seized {} GOR. Remaining debt: {}, remaining collateral: {}",
            repay_amount,
            collateral_to_liquidator,
            vault.debt_amount,
            vault.collateral_amount
        );

        // [MEDIUM-3] Emit event
        emit!(VaultLiquidated {
            vault_owner: vault_owner_key,
            liquidator: ctx.accounts.liquidator.key(),
            debt_repaid: repay_amount,
            collateral_seized: collateral_to_liquidator,
        });

        Ok(())
    }

    /// Get vault health metrics (view function)
    pub fn get_vault_health(ctx: Context<GetVaultHealth>) -> Result<VaultHealth> {
        let vault = &ctx.accounts.vault;
        let protocol = &ctx.accounts.protocol_state;

        let collateral_value_usd = calculate_usd_value(
            vault.collateral_amount,
            protocol.gor_price_usd,
            GOR_DECIMALS,
        )?;

        let collateral_ratio = if vault.debt_amount > 0 {
            collateral_value_usd
                .checked_mul(BPS_DENOMINATOR)
                .ok_or(GusdError::MathOverflow)?
                .checked_div(vault.debt_amount)
                .ok_or(GusdError::MathOverflow)?
        } else {
            u64::MAX // No debt = infinite ratio
        };

        let is_liquidatable = vault.debt_amount > 0 && 
            collateral_ratio < LIQUIDATION_THRESHOLD_BPS;

        let health = VaultHealth {
            collateral_amount: vault.collateral_amount,
            collateral_value_usd,
            debt_amount: vault.debt_amount,
            collateral_ratio_bps: collateral_ratio,
            is_liquidatable,
        };

        msg!("Vault Health:");
        msg!("  Collateral: {} GOR (${:.2})", 
            vault.collateral_amount as f64 / 1e9,
            collateral_value_usd as f64 / 1e6
        );
        msg!("  Debt: {} GUSD", vault.debt_amount as f64 / 1e6);
        msg!("  Ratio: {}%", collateral_ratio as f64 / 100.0);
        msg!("  Liquidatable: {}", is_liquidatable);

        Ok(health)
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Calculate USD value of GOR amount
/// [HIGH-1] Fixed: Now checks for u128 -> u64 overflow
fn calculate_usd_value(gor_amount: u64, gor_price_usd: u64, gor_decimals: u8) -> Result<u64> {
    // gor_amount is in lamports (10^-9)
    // gor_price_usd has 6 decimals
    // Result should have 6 decimals (GUSD decimals)
    
    let value = (gor_amount as u128)
        .checked_mul(gor_price_usd as u128)
        .ok_or(GusdError::MathOverflow)?
        .checked_div(10u128.pow(gor_decimals as u32))
        .ok_or(GusdError::MathOverflow)?;
    
    // [HIGH-1] Add overflow check
    require!(value <= u64::MAX as u128, GusdError::MathOverflow);
    
    Ok(value as u64)
}

// ============================================================================
// ACCOUNTS
// ============================================================================

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + ProtocolState::INIT_SPACE,
        seeds = [b"protocol"],
        bump
    )]
    pub protocol_state: Account<'info, ProtocolState>,

    #[account(
        init,
        payer = admin,
        mint::decimals = GUSD_DECIMALS,
        mint::authority = protocol_state,
        mint::freeze_authority = protocol_state,
        seeds = [b"gusd_mint"],
        bump
    )]
    pub gusd_mint: InterfaceAccount<'info, Mint>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdatePrice<'info> {
    #[account(
        constraint = admin.key() == protocol_state.admin @ GusdError::Unauthorized
    )]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [b"protocol"],
        bump = protocol_state.bump
    )]
    pub protocol_state: Account<'info, ProtocolState>,
}

/// [LOW-2] Admin transfer accounts struct
#[derive(Accounts)]
pub struct TransferAdmin<'info> {
    #[account(
        constraint = admin.key() == protocol_state.admin @ GusdError::Unauthorized
    )]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [b"protocol"],
        bump = protocol_state.bump
    )]
    pub protocol_state: Account<'info, ProtocolState>,
}

/// [CRITICAL-4] Fixed: Now initializes vault_collateral PDA
#[derive(Accounts)]
pub struct CreateVault<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        init,
        payer = owner,
        space = 8 + Vault::INIT_SPACE,
        seeds = [b"vault", owner.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, Vault>,

    /// [CRITICAL-4] Initialize the vault_collateral PDA
    #[account(
        init,
        payer = owner,
        space = 0,
        seeds = [b"vault_collateral", owner.key().as_ref()],
        bump
    )]
    /// CHECK: PDA that holds GOR collateral as lamports
    pub vault_collateral: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DepositCollateral<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", owner.key().as_ref()],
        bump = vault.bump,
        constraint = vault.owner == owner.key() @ GusdError::Unauthorized
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        seeds = [b"vault_collateral", owner.key().as_ref()],
        bump
    )]
    /// CHECK: This is a PDA that holds lamports (GOR)
    pub vault_collateral: AccountInfo<'info>,

    #[account(
        mut,
        seeds = [b"protocol"],
        bump = protocol_state.bump
    )]
    pub protocol_state: Account<'info, ProtocolState>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct MintGusd<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", owner.key().as_ref()],
        bump = vault.bump,
        constraint = vault.owner == owner.key() @ GusdError::Unauthorized
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        seeds = [b"protocol"],
        bump = protocol_state.bump
    )]
    pub protocol_state: Account<'info, ProtocolState>,

    #[account(
        mut,
        seeds = [b"gusd_mint"],
        bump = protocol_state.mint_bump
    )]
    pub gusd_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init_if_needed,
        payer = owner,
        associated_token::mint = gusd_mint,
        associated_token::authority = owner,
        associated_token::token_program = token_program
    )]
    pub user_gusd_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RepayGusd<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", owner.key().as_ref()],
        bump = vault.bump,
        constraint = vault.owner == owner.key() @ GusdError::Unauthorized
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        seeds = [b"protocol"],
        bump = protocol_state.bump
    )]
    pub protocol_state: Account<'info, ProtocolState>,

    #[account(
        mut,
        seeds = [b"gusd_mint"],
        bump = protocol_state.mint_bump
    )]
    pub gusd_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        associated_token::mint = gusd_mint,
        associated_token::authority = owner,
        associated_token::token_program = token_program
    )]
    pub user_gusd_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct WithdrawCollateral<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", owner.key().as_ref()],
        bump = vault.bump,
        constraint = vault.owner == owner.key() @ GusdError::Unauthorized
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        seeds = [b"vault_collateral", owner.key().as_ref()],
        bump
    )]
    /// CHECK: This is a PDA that holds lamports (GOR)
    pub vault_collateral: AccountInfo<'info>,

    #[account(
        mut,
        seeds = [b"protocol"],
        bump = protocol_state.bump
    )]
    pub protocol_state: Account<'info, ProtocolState>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]


#[derive(Accounts)]
pub struct CloseVault<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        mut,
        close = owner,
        seeds = [b"vault", owner.key().as_ref()],
        bump = vault.bump,
        constraint = vault.owner == owner.key() @ GusdError::InvalidVaultOwner
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        seeds = [b"vault_collateral", owner.key().as_ref()],
        bump = vault.collateral_bump
    )]
    /// CHECK: PDA that holds GOR collateral as lamports (0-data account)
    pub vault_collateral: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}
pub struct Liquidate<'info> {
    #[account(mut)]
    pub liquidator: Signer<'info>,

    /// CHECK: The owner of the vault being liquidated
    pub vault_owner: AccountInfo<'info>,

    /// [HIGH-2] Fixed: Added vault owner constraint
    #[account(
        mut,
        seeds = [b"vault", vault_owner.key().as_ref()],
        bump = vault.bump,
        constraint = vault.owner == vault_owner.key() @ GusdError::InvalidVaultOwner
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        mut,
        seeds = [b"vault_collateral", vault_owner.key().as_ref()],
        bump
    )]
    /// CHECK: This is a PDA that holds lamports (GOR)
    pub vault_collateral: AccountInfo<'info>,

    #[account(
        mut,
        seeds = [b"protocol"],
        bump = protocol_state.bump
    )]
    pub protocol_state: Account<'info, ProtocolState>,

    #[account(
        mut,
        seeds = [b"gusd_mint"],
        bump = protocol_state.mint_bump
    )]
    pub gusd_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        associated_token::mint = gusd_mint,
        associated_token::authority = liquidator,
        associated_token::token_program = token_program
    )]
    pub liquidator_gusd_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct GetVaultHealth<'info> {
    /// CHECK: Can query any vault
    pub vault_owner: AccountInfo<'info>,

    #[account(
        seeds = [b"vault", vault_owner.key().as_ref()],
        bump = vault.bump
    )]
    pub vault: Account<'info, Vault>,

    #[account(
        seeds = [b"protocol"],
        bump = protocol_state.bump
    )]
    pub protocol_state: Account<'info, ProtocolState>,
}

// ============================================================================
// STATE
// ============================================================================

/// [MEDIUM-2] Updated: Added is_paused field
#[account]
#[derive(InitSpace)]
pub struct ProtocolState {
    /// Protocol admin (can update price for MVP)
    pub admin: Pubkey,
    /// GUSD mint address
    pub gusd_mint: Pubkey,
    /// Current GOR price in USD (6 decimals, e.g., 1_000_000 = $1.00)
    pub gor_price_usd: u64,
    /// Total GOR collateral locked in protocol
    pub total_collateral: u64,
    /// Total GUSD debt outstanding
    pub total_debt: u64,
    /// PDA bump
    pub bump: u8,
    /// Mint PDA bump
    pub mint_bump: u8,
    /// [MEDIUM-2] Protocol pause state
    pub is_paused: bool,
    /// Timestamp of last price update (unix seconds)
    pub last_price_update_ts: i64,
}

/// [CRITICAL-4] Updated: Added collateral_bump field
#[account]
#[derive(InitSpace)]
pub struct Vault {
    /// Owner of this vault
    pub owner: Pubkey,
    /// Amount of GOR collateral (in lamports)
    pub collateral_amount: u64,
    /// Amount of GUSD debt (in GUSD smallest unit, 6 decimals)
    pub debt_amount: u64,
    /// PDA bump
    pub bump: u8,
    /// [CRITICAL-4] Collateral PDA bump
    pub collateral_bump: u8,
}

// ============================================================================
// RETURN TYPES
// ============================================================================

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct VaultHealth {
    pub collateral_amount: u64,
    pub collateral_value_usd: u64,
    pub debt_amount: u64,
    pub collateral_ratio_bps: u64,
    pub is_liquidatable: bool,
}

// ============================================================================
// ERRORS
// ============================================================================

#[error_code]
pub enum GusdError {
    #[msg("Unauthorized access")]
    Unauthorized,
    #[msg("Invalid amount")]
    InvalidAmount,
    #[msg("Invalid price")]
    InvalidPrice,
    #[msg("Insufficient collateral for this operation")]
    InsufficientCollateral,
    #[msg("This withdrawal would undercollateralize the vault")]
    WouldUndercollateralize,
    #[msg("Vault is not eligible for liquidation")]
    VaultNotLiquidatable,
    #[msg("No debt to liquidate")]
    NoDebtToLiquidate,
    #[msg("Liquidation is not profitable with current collateral")]
    LiquidationNotProfitable,
    #[msg("Math overflow")]
    MathOverflow,
    /// [HIGH-2] New error
    #[msg("Invalid vault owner")]
    InvalidVaultOwner,
    /// [MEDIUM-1] New error
    #[msg("Price change exceeds maximum allowed limit")]
    PriceChangeExceedsLimit,
    #[msg("Price update is too frequent")]
    PriceUpdateTooFrequent,
    /// [MEDIUM-2] New error
    #[msg("Protocol is paused")]
    ProtocolPaused,
    #[msg("Vault must have zero debt and zero collateral")]
    VaultNotEmpty,
}

// ============================================================================
// EVENTS [MEDIUM-3]
// ============================================================================

#[event]
pub struct VaultCreated {
    pub owner: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct CollateralDeposited {
    pub owner: Pubkey,
    pub amount: u64,
    pub total_collateral: u64,
}

#[event]
pub struct GusdMinted {
    pub owner: Pubkey,
    pub amount: u64,
    pub total_debt: u64,
    pub collateral_ratio_bps: u64,
}

#[event]
pub struct GusdRepaid {
    pub owner: Pubkey,
    pub amount: u64,
    pub remaining_debt: u64,
}

#[event]
pub struct CollateralWithdrawn {
    pub owner: Pubkey,
    pub amount: u64,
    pub remaining_collateral: u64,
}

#[event]
pub struct VaultLiquidated {
    pub vault_owner: Pubkey,
    pub liquidator: Pubkey,
    pub debt_repaid: u64,
    pub collateral_seized: u64,
}

#[event]
pub struct PriceUpdated {
    pub old_price: u64,
    pub new_price: u64,
}
