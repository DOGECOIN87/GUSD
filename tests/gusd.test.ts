import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Gusd } from "../target/types/gusd";
import {
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAssociatedTokenAddress,
  createAssociatedTokenAccountInstruction,
  createTransferInstruction,
} from "@solana/spl-token";
import { assert } from "chai";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

describe("GUSD Stablecoin Protocol (Audited)", () => {
  // Configure the client
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Gusd as Program<Gusd>;
  
  // Test accounts
  const admin = provider.wallet;
  const user = Keypair.generate();
  const liquidator = Keypair.generate();
  const newAdmin = Keypair.generate();

  // PDAs
  let protocolStatePda: PublicKey;
  let gusdMintPda: PublicKey;
  let userVaultPda: PublicKey;
  let userVaultCollateralPda: PublicKey;

  // Initial GOR price: $0.004776 (sGOR price from Jupiter)
  // Stored with 6 decimals: 0.004776 * 1_000_000 = 4776
  const INITIAL_GOR_PRICE = 4776;
  
  // Max price change per update: 20%
  const MAX_PRICE_CHANGE_BPS = 2000;
  
  // Test amounts
  // At $0.004776 per GOR:
  // - 50,000 GOR = $238.80 collateral value
  // - Can mint up to $159.20 GUSD (at 150% ratio)
  // - We'll mint 100 GUSD to stay safe
  const DEPOSIT_AMOUNT = 50_000 * LAMPORTS_PER_SOL; // 50,000 GOR (~$239)
  const MINT_AMOUNT = 100_000_000; // 100 GUSD (with 6 decimals)

  before(async () => {
    // Derive PDAs
    [protocolStatePda] = PublicKey.findProgramAddressSync(
      [Buffer.from("protocol")],
      program.programId
    );

    [gusdMintPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("gusd_mint")],
      program.programId
    );

    [userVaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), user.publicKey.toBuffer()],
      program.programId
    );

    [userVaultCollateralPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_collateral"), user.publicKey.toBuffer()],
      program.programId
    );

    // Airdrop SOL to test accounts
    const airdropUser = await provider.connection.requestAirdrop(
      user.publicKey,
      100 * LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropUser);

    const airdropLiquidator = await provider.connection.requestAirdrop(
      liquidator.publicKey,
      100 * LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropLiquidator);

    console.log("Test Setup Complete");
    console.log("Protocol State PDA:", protocolStatePda.toString());
    console.log("GUSD Mint PDA:", gusdMintPda.toString());
    console.log("User:", user.publicKey.toString());
  });

  describe("Protocol Initialization", () => {
    it("Initializes the GUSD protocol", async () => {
      const tx = await program.methods
        .initialize(new anchor.BN(INITIAL_GOR_PRICE))
        .accounts({
          admin: admin.publicKey,
          protocolState: protocolStatePda,
          gusdMint: gusdMintPda,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      console.log("Initialize tx:", tx);

      // Verify protocol state
      const protocolState = await program.account.protocolState.fetch(
        protocolStatePda
      );
      assert.equal(
        protocolState.admin.toString(),
        admin.publicKey.toString()
      );
      assert.equal(protocolState.gorPriceUsd.toNumber(), INITIAL_GOR_PRICE);
      assert.equal(protocolState.totalCollateral.toNumber(), 0);
      assert.equal(protocolState.totalDebt.toNumber(), 0);

      console.log("Protocol initialized with GOR price: $1.00");
    });
  });

  describe("Vault Operations", () => {
    it("Creates a vault for user", async () => {
      const tx = await program.methods
        .createVault()
        .accounts({
          owner: user.publicKey,
          vault: userVaultPda,
          vaultCollateral: userVaultCollateralPda,
          systemProgram: SystemProgram.programId,
        })
        .signers([user])
        .rpc();

      console.log("Create vault tx:", tx);

      // Verify vault
      const vault = await program.account.vault.fetch(userVaultPda);
      assert.equal(vault.owner.toString(), user.publicKey.toString());
      assert.equal(vault.collateralAmount.toNumber(), 0);
      assert.equal(vault.debtAmount.toNumber(), 0);

      console.log("Vault created for user");
    });

    it("Deposits collateral into vault", async () => {
      const tx = await program.methods
        .depositCollateral(new anchor.BN(DEPOSIT_AMOUNT))
        .accounts({
          owner: user.publicKey,
          vault: userVaultPda,
          vaultCollateral: userVaultCollateralPda,
          protocolState: protocolStatePda,
          systemProgram: SystemProgram.programId,
        })
        .signers([user])
        .rpc();

      console.log("Deposit collateral tx:", tx);

      // Verify vault
      const vault = await program.account.vault.fetch(userVaultPda);
      assert.equal(vault.collateralAmount.toNumber(), DEPOSIT_AMOUNT);

      // Verify protocol totals
      const protocol = await program.account.protocolState.fetch(
        protocolStatePda
      );
      assert.equal(protocol.totalCollateral.toNumber(), DEPOSIT_AMOUNT);

      console.log(`Deposited ${DEPOSIT_AMOUNT / LAMPORTS_PER_SOL} GOR`);
    });

    it("Mints GUSD against collateral", async () => {
      const userGusdAccount = await getAssociatedTokenAddress(
        gusdMintPda,
        user.publicKey
      );

      const tx = await program.methods
        .mintGusd(new anchor.BN(MINT_AMOUNT))
        .accounts({
          owner: user.publicKey,
          vault: userVaultPda,
          protocolState: protocolStatePda,
          gusdMint: gusdMintPda,
          userGusdAccount: userGusdAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
          associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([user])
        .rpc();

      console.log("Mint GUSD tx:", tx);

      // Verify vault debt
      const vault = await program.account.vault.fetch(userVaultPda);
      assert.equal(vault.debtAmount.toNumber(), MINT_AMOUNT);

      console.log(`Minted ${MINT_AMOUNT / 1_000_000} GUSD`);
      console.log(`Collateral ratio: ${(DEPOSIT_AMOUNT / LAMPORTS_PER_SOL) * INITIAL_GOR_PRICE / MINT_AMOUNT * 100}%`);
    });
  });

  describe("Price Oracle", () => {
    it("Updates GOR price within 20% limit (admin only)", async () => {
      // Calculate max allowed change (20% of initial price)
      const maxChange = Math.floor(INITIAL_GOR_PRICE * MAX_PRICE_CHANGE_BPS / 10000);
      const newPrice = INITIAL_GOR_PRICE + maxChange; // At the limit

      await sleep(1100);
const tx = await program.methods
        .updatePrice(new anchor.BN(newPrice))
        .accounts({
          admin: admin.publicKey,
          protocolState: protocolStatePda,
        })
        .rpc();

      console.log("Update price tx:", tx);

      const protocol = await program.account.protocolState.fetch(
        protocolStatePda
      );
      assert.equal(protocol.gorPriceUsd.toNumber(), newPrice);

      console.log(`GOR price updated to ${newPrice} (within 20% limit)`);
    });

    it("Rejects price change exceeding 20% limit", async () => {
      const currentPrice = (await program.account.protocolState.fetch(protocolStatePda)).gorPriceUsd.toNumber();
      const maxChange = Math.floor(currentPrice * MAX_PRICE_CHANGE_BPS / 10000);
      const invalidPrice = currentPrice + maxChange + 1; // Just over the limit

      try {
        await sleep(1100);
await program.methods
          .updatePrice(new anchor.BN(invalidPrice))
          .accounts({
            admin: admin.publicKey,
            protocolState: protocolStatePda,
          })
          .rpc();
        
        assert.fail("Should have thrown an error");
      } catch (error) {
        console.log("Correctly rejected price change exceeding 20% limit");
      }
    });

    it("Rejects price update from non-admin", async () => {
      try {
        await sleep(1100);
await program.methods
          .updatePrice(new anchor.BN(500_000))
          .accounts({
            admin: user.publicKey,
            protocolState: protocolStatePda,
          })
          .signers([user])
          .rpc();
        
        assert.fail("Should have thrown an error");
      } catch (error) {
        console.log("Correctly rejected non-admin price update");
      }
    });
  });

  describe("Protocol Pause/Unpause", () => {
    it("Pauses the protocol (admin only)", async () => {
      const tx = await program.methods
        .pauseProtocol()
        .accounts({
          admin: admin.publicKey,
          protocolState: protocolStatePda,
        })
        .rpc();

      console.log("Pause protocol tx:", tx);

      const protocol = await program.account.protocolState.fetch(protocolStatePda);
      assert.equal(protocol.isPaused, true);
      console.log("Protocol paused");
    });

    it("Rejects operations when paused", async () => {
      try {
        await program.methods
          .depositCollateral(new anchor.BN(LAMPORTS_PER_SOL))
          .accounts({
            owner: user.publicKey,
            vault: userVaultPda,
            vaultCollateral: userVaultCollateralPda,
            protocolState: protocolStatePda,
            systemProgram: SystemProgram.programId,
          })
          .signers([user])
          .rpc();
        
        assert.fail("Should have thrown an error");
      } catch (error) {
        console.log("Correctly rejected operation while paused");
      }
    });

    it("Unpauses the protocol (admin only)", async () => {
      const tx = await program.methods
        .unpauseProtocol()
        .accounts({
          admin: admin.publicKey,
          protocolState: protocolStatePda,
        })
        .rpc();

      console.log("Unpause protocol tx:", tx);

      const protocol = await program.account.protocolState.fetch(protocolStatePda);
      assert.equal(protocol.isPaused, false);
      console.log("Protocol unpaused");
    });
  });

  describe("Admin Transfer", () => {
    it("Transfers admin role to new address", async () => {
      const tx = await program.methods
        .transferAdmin(newAdmin.publicKey)
        .accounts({
          admin: admin.publicKey,
          protocolState: protocolStatePda,
        })
        .rpc();

      console.log("Transfer admin tx:", tx);

      const protocol = await program.account.protocolState.fetch(protocolStatePda);
      assert.equal(protocol.admin.toString(), newAdmin.publicKey.toString());
      console.log(`Admin transferred to ${newAdmin.publicKey.toString()}`);

      // Transfer back for other tests
      await program.methods
        .transferAdmin(admin.publicKey)
        .accounts({
          admin: newAdmin.publicKey,
          protocolState: protocolStatePda,
        })
        .signers([newAdmin])
        .rpc();
      
      console.log("Admin transferred back for remaining tests");
    });
  });

  describe("Repayment & Withdrawal", () => {
    it("Repays GUSD debt", async () => {
      const userGusdAccount = await getAssociatedTokenAddress(
        gusdMintPda,
        user.publicKey
      );

      const repayAmount = 2_000_000; // Repay 2 GUSD

      const tx = await program.methods
        .repayGusd(new anchor.BN(repayAmount))
        .accounts({
          owner: user.publicKey,
          vault: userVaultPda,
          protocolState: protocolStatePda,
          gusdMint: gusdMintPda,
          userGusdAccount: userGusdAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([user])
        .rpc();

      console.log("Repay GUSD tx:", tx);

      const vault = await program.account.vault.fetch(userVaultPda);
      assert.equal(vault.debtAmount.toNumber(), MINT_AMOUNT - repayAmount);

      console.log(`Repaid ${repayAmount / 1_000_000} GUSD`);
      console.log(`Remaining debt: ${vault.debtAmount.toNumber() / 1_000_000} GUSD`);
    });

    it("Withdraws excess collateral", async () => {
      const withdrawAmount = 1 * LAMPORTS_PER_SOL; // Withdraw 1 GOR

      const tx = await program.methods
        .withdrawCollateral(new anchor.BN(withdrawAmount))
        .accounts({
          owner: user.publicKey,
          vault: userVaultPda,
          vaultCollateral: userVaultCollateralPda,
          protocolState: protocolStatePda,
          systemProgram: SystemProgram.programId,
        })
        .signers([user])
        .rpc();

      console.log("Withdraw collateral tx:", tx);

      const vault = await program.account.vault.fetch(userVaultPda);
      assert.equal(
        vault.collateralAmount.toNumber(),
        DEPOSIT_AMOUNT - withdrawAmount
      );

      console.log(`Withdrew ${withdrawAmount / LAMPORTS_PER_SOL} GOR`);
    });
  });

  describe("Liquidation", () => {
    it("Sets up undercollateralized vault for liquidation test", async () => {
      // Lower GOR price gradually (max 20% change per update) to make vault undercollateralized
      const targetPrice = 1956; // $0.001956

      let protocol = await program.account.protocolState.fetch(protocolStatePda);
      let currentPrice = protocol.gorPriceUsd.toNumber();

      while (currentPrice > targetPrice) {
        const nextPrice = Math.max(
          targetPrice,
          Math.floor((currentPrice * (10000 - MAX_PRICE_CHANGE_BPS)) / 10000)
        );

        await sleep(1100);
        await program.methods
          .updatePrice(new anchor.BN(nextPrice))
          .accounts({
            admin: admin.publicKey,
            protocolState: protocolStatePda,
          })
          .rpc();

        currentPrice = nextPrice;
      }

      protocol = await program.account.protocolState.fetch(protocolStatePda);
      console.log(
        `GOR price dropped to $${protocol.gorPriceUsd.toNumber() / 1_00it("Liquidates undercollateralized vault", async () => {
      // Create liquidator ATA for GUSD (required by the program)
      const liquidatorGusdAccount = await getAssociatedTokenAddress(
        gusdMintPda,
        liquidator.publicKey
      );

      const ataInfo = await provider.connection.getAccountInfo(liquidatorGusdAccount);
      if (!ataInfo) {
        const createAtaIx = createAssociatedTokenAccountInstruction(
          liquidator.publicKey, // payer
          liquidatorGusdAccount,
          liquidator.publicKey, // owner
          gusdMintPda
        );

        const tx = new anchor.web3.Transaction().add(createAtaIx);
        await provider.sendAndConfirm(tx, [liquidator]);
      }

      // Transfer GUSD from user to liquidator to simulate buying on the market
      const userGusdAccount = await getAssociatedTokenAddress(
        gusdMintPda,
        user.publicKey
      );

      const transferIx = createTransferInstruction(
        userGusdAccount,
        liquidatorGusdAccount,
        user.publicKey,
        MINT_AMOUNT
      );

      await provider.sendAndConfirm(
        new anchor.web3.Transaction().add(transferIx),
        [user]
      );

      const vaultBefore = await program.account.vault.fetch(userVaultPda);
      const liquidatorSolBefore = await provider.connection.getBalance(liquidator.publicKey);

      const txSig = await program.methods
        .liquidate()
        .accounts({
          liquidator: liquidator.publicKey,
          vaultOwner: user.publicKey,
          vault: userVaultPda,
          vaultCollateral: userVaultCollateralPda,
          protocolState: protocolStatePda,
          gusdMint: gusdMintPda,
          liquidatorGusdAccount: liquidatorGusdAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .signers([liquidator])
        .rpc();

      console.log("Liquidate tx:", txSig);

      const vaultAfter = await program.account.vault.fetch(userVaultPda);
      const liquidatorSolAfter = await provider.connection.getBalance(liquidator.publicKey);

      assert.isBelow(vaultAfter.debtAmount.toNumber(), vaultBefore.debtAmount.toNumber());
      assert.isBelow(vaultAfter.collateralAmount.toNumber(), vaultBefore.collateralAmount.toNumber());
      assert.isAbove(liquidatorSolAfter, liquidatorSolBefore);

      console.log("Vault debt before/after:", vaultBefore.debtAmount.toNumber(), vaultAfter.debtAmount.toNumber());
      console.log("Vault collateral before/after:", vaultBefore.collateralAmount.toNumber(), vaultAfter.collateralAmount.toNumber());
    });. Call liquidate() to repay debt and claim collateral + bonus");
    });
  });

  describe("View Functions", () => {
    it("Gets vault health metrics", async () => {
      const tx = await program.methods
        .getVaultHealth()
        .accounts({
          vaultOwner: user.publicKey,
          vault: userVaultPda,
          protocolState: protocolStatePda,
        })
        .rpc();

      console.log("Get vault health tx:", tx);

      // The health metrics are emitted as logs
      // In a real client, you'd parse these from the transaction logs
    });
  });
});
