# GUSD Stablecoin Protocol for Gorbagana

A crypto-collateralized stablecoin protocol built on Gorbagana (Solana fork). Users can deposit GOR as collateral and mint GUSD, a USD-pegged stablecoin.

**Status:** ✅ Audited & Revised (December 2024)

## Token Context

| Token | Chain | Description |
|-------|-------|-------------|
| **sGOR** | Solana | Original GOR token ([Jupiter](https://jup.ag/tokens/71Jvq4Epe2FCJ7JFSF7jLXdNk1Wy4Bhqd9iL6bEFELvg)) |
| **gGOR / GOR** | Gorbagana | Native chain token, bridged from sGOR (23% of supply) |
| **GUSD** | Gorbagana | USD stablecoin, backed by GOR collateral |

**Current sGOR Price**: ~$0.0048

The protocol uses sGOR price as the oracle reference for gGOR valuation.

## Security Features (Post-Audit)

| Feature | Description |
|---------|-------------|
| **Emergency Pause** | Admin can pause/unpause protocol in emergencies |
| **Price Change Limits** | Max 20% price change per update to prevent manipulation |
| **PDA-Signed Transfers** | All collateral transfers use proper PDA signatures |
| **Overflow Protection** | u128 → u64 conversions are explicitly checked |
| **Event Emission** | All operations emit events for indexing/monitoring |
| **Admin Transfer** | Admin role can be transferred to multisig/DAO |

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                    GUSD Protocol Architecture                    │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐     │
│  │    User      │────▶│    Vault     │────▶│   Protocol   │     │
│  │   Wallet     │     │   Account    │     │    State     │     │
│  └──────────────┘     └──────────────┘     └──────────────┘     │
│         │                    │                    │              │
│         │              Collateral              Oracle            │
│         │               (GOR)                  Price             │
│         │                    │                    │              │
│         │                    ▼                    ▼              │
│         │             ┌──────────────┐    ┌──────────────┐      │
│         └────────────▶│  GUSD Mint   │◀───│   Burn on    │      │
│           Receives    │   (SPL)      │    │   Repay      │      │
│            GUSD       └──────────────┘    └──────────────┘      │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## How It Works

### Minting GUSD
1. User deposits GOR collateral into their vault
2. User can mint GUSD up to 66% of collateral value (150% collateral ratio)
3. GUSD is minted to user's wallet

### Maintaining Peg
- **Over-collateralization**: Every $1 of GUSD is backed by $1.50+ of GOR
- **Liquidation**: Underwater vaults are liquidated, maintaining system solvency
- **Arbitrage**: If GUSD < $1, buy GUSD cheap → repay debt → profit. If GUSD > $1, mint new GUSD → sell for profit

### Liquidation
- Vaults below 120% collateral ratio can be liquidated
- Liquidators repay the debt and receive collateral + 10% bonus
- This incentivizes keeping the system healthy

## Protocol Parameters

| Parameter | Value | Description |
|-----------|-------|-------------|
| Min Collateral Ratio | 150% | Required ratio to mint GUSD |
| Liquidation Threshold | 120% | Ratio below which liquidation is allowed |
| Liquidation Penalty | 10% | Bonus for liquidators |
| GUSD Decimals | 6 | Same as USDC |
| GOR Decimals | 9 | Same as SOL |

## Project Structure

```
gusd-stablecoin/
├── Anchor.toml              # Anchor configuration
├── Cargo.toml               # Rust workspace
├── package.json             # Node dependencies
├── tsconfig.json            # TypeScript config
├── programs/
│   └── gusd/
│       ├── Cargo.toml       # Program dependencies
│       └── src/
│           └── lib.rs       # Main program logic
├── tests/
│   └── gusd.test.ts         # Integration tests
└── app/                     # Frontend (optional)
```

## Instructions

### `initialize`
Creates the protocol state and GUSD mint. Admin only, called once.

```rust
pub fn initialize(ctx: Context<Initialize>, initial_gor_price_usd: u64) -> Result<()>
```

### `update_price`
Updates the GOR/USD price. Admin only (MVP). Max 20% change per update. Replace with oracle for production.

```rust
pub fn update_price(ctx: Context<UpdatePrice>, new_gor_price_usd: u64) -> Result<()>
```

### `pause_protocol` / `unpause_protocol`
Emergency pause/unpause. Admin only. Blocks deposits, mints, repays, withdrawals, and liquidations.

```rust
pub fn pause_protocol(ctx: Context<UpdatePrice>) -> Result<()>
pub fn unpause_protocol(ctx: Context<UpdatePrice>) -> Result<()>
```

### `transfer_admin`
Transfer admin role to new address (e.g., multisig or DAO).

```rust
pub fn transfer_admin(ctx: Context<TransferAdmin>, new_admin: Pubkey) -> Result<()>
```

### `create_vault`
Creates a vault for a user to store collateral and track debt.

```rust
pub fn create_vault(ctx: Context<CreateVault>) -> Result<()>
```

### `deposit_collateral`
Deposits GOR into user's vault.

```rust
pub fn deposit_collateral(ctx: Context<DepositCollateral>, amount: u64) -> Result<()>
```

### `mint_gusd`
Mints GUSD against deposited collateral. Checks collateral ratio.

```rust
pub fn mint_gusd(ctx: Context<MintGusd>, amount: u64) -> Result<()>
```

### `repay_gusd`
Burns GUSD to reduce debt.

```rust
pub fn repay_gusd(ctx: Context<RepayGusd>, amount: u64) -> Result<()>
```

### `withdraw_collateral`
Withdraws GOR from vault. Checks that ratio stays healthy.

```rust
pub fn withdraw_collateral(ctx: Context<WithdrawCollateral>, amount: u64) -> Result<()>
```

### `liquidate`
Liquidates an undercollateralized vault. Anyone can call.

```rust
pub fn liquidate(ctx: Context<Liquidate>) -> Result<()>
```

## Getting Started

### Prerequisites
- Rust 1.75+
- Solana CLI 1.18+
- Anchor CLI 0.30+
- Node.js 18+

### Build
```bash
# Install dependencies
yarn install

# Build the program
anchor build
```

### Test
```bash
# Run tests on localnet
anchor test
```

### Deploy
```bash
# Deploy to devnet
anchor deploy --provider.cluster devnet

# Deploy to mainnet (Gorbagana)
anchor deploy --provider.cluster mainnet
```

## Example Usage (TypeScript)

```typescript
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Gusd } from "./target/types/gusd";

// Initialize provider
const provider = anchor.AnchorProvider.env();
anchor.setProvider(provider);
const program = anchor.workspace.Gusd as Program<Gusd>;

// Create vault
await program.methods
  .createVault()
  .accounts({ owner: wallet.publicKey, ... })
  .rpc();

// Deposit 50,000 GOR (~$239 at $0.0048/GOR)
await program.methods
  .depositCollateral(new anchor.BN(50_000 * LAMPORTS_PER_SOL))
  .accounts({ owner: wallet.publicKey, ... })
  .rpc();

// Mint 100 GUSD (well within 150% collateral ratio)
// Max mintable: $239 / 1.5 = ~$159 GUSD
await program.methods
  .mintGusd(new anchor.BN(100_000_000)) // 100 GUSD with 6 decimals
  .accounts({ owner: wallet.publicKey, ... })
  .rpc();
```

## Collateral Examples (at $0.0048/GOR)

| GOR Deposited | USD Value | Max GUSD Mintable (150% ratio) |
|---------------|-----------|-------------------------------|
| 10,000 GOR | $48 | 32 GUSD |
| 50,000 GOR | $240 | 160 GUSD |
| 100,000 GOR | $480 | 320 GUSD |
| 500,000 GOR | $2,400 | 1,600 GUSD |
| 1,000,000 GOR | $4,800 | 3,200 GUSD |

## Production Considerations

### 1. Oracle Integration
Replace admin-controlled price with TrashBin DEX oracle:
- **Phase 1**: Admin-controlled (mirrors sGOR price from Solana)
- **Phase 2**: Create GOR/GUSD pool on TrashBin (trashbin.fun)
- **Phase 3**: Read price directly from TrashBin pool reserves

### 2. Partial Liquidations
Upgrade to partial liquidations for better UX:
- Only liquidate enough to restore healthy ratio
- Users keep remaining collateral

### 3. Stability Fee
Add interest on borrowed GUSD:
- Accumulates over time
- Protocol revenue
- Can adjust to influence supply/demand

### 4. Governance
Add governance for parameter changes:
- Collateral ratios
- Liquidation penalties
- Adding new collateral types

### 5. Multi-Collateral
Support multiple collateral types:
- Different assets with different risk parameters
- Diversified backing

### 6. Emergency Shutdown
Add admin emergency functions:
- Pause minting
- Global settlement
- Asset recovery

## Security Notes

✅ **This code has been audited and revised (December 2024)**

Fixes applied:
- [x] PDA-signed transfers for collateral movements
- [x] u128 → u64 overflow protection
- [x] Correct liquidation math
- [x] Emergency pause mechanism
- [x] Price change limits (20% max)
- [x] Event emission for monitoring
- [x] Admin transfer capability

Before mainnet:
- [ ] Professional third-party security audit (recommended)
- [ ] Extensive testnet deployment
- [ ] Bug bounty program
- [ ] Gradual rollout with caps

## License

MIT
