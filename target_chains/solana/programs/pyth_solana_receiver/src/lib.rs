pub mod error;
pub mod state;

// official wormhole sdk
use {
    crate::error::ReceiverError,
    anchor_lang::{
        prelude::*,
        solana_program::{
            instruction::Instruction,
            sysvar::SysvarId,
        },
    },
    hex::ToHex,
    pyth_wormhole_attester_sdk::BatchPriceAttestation,
    pythnet_sdk::{
        accumulators::merkle::MerkleRoot,
        hashers::keccak256_160::Keccak160,
        messages::Message,
        wire::{
            from_slice,
            v1::{
                Proof,
                WormholeMessage,
                WormholePayload,
            },
        },
    },
    serde::Deserialize,
    sha3::Digest,
    solana_program::{
        keccak,
        secp256k1_recover::secp256k1_recover,
    },
    state::AnchorVaa,
    std::io::Write,
    wormhole::Chain::{
        self,
        Pythnet,
        Solana,
    },
    wormhole_anchor_sdk::{
        wormhole as wormhole_anchor,
        wormhole::SEED_PREFIX_POSTED_VAA,
    },
    wormhole_sdk::Vaa,
};

// declare_id!("pythKkWXoywbvTQVcWrNDz5ENvWteF7tem7xzW52NBK");
declare_id!("DvPfMBZJJwKgJsv2WJA8bFwUMn8nFd5Xpioc6foC3rse");
pub const POST_VAA: u8 = 2;

#[program]
pub mod pyth_solana_receiver {
    use {
        super::*,
        pythnet_sdk::wire::v1::{
            AccumulatorUpdateData,
            MerklePriceUpdate,
        },
        serde_wormhole::RawMessage,
        solana_program::program::invoke,
        std::hash::Hash,
        wormhole_sdk::vaa::{
            Body,
            Header,
        },
    };

    pub fn decode_posted_vaa(ctx: Context<DecodePostedVaa>) -> Result<()> {
        let posted_vaa = &ctx.accounts.posted_vaa.payload;
        let batch: BatchPriceAttestation =
            BatchPriceAttestation::deserialize(posted_vaa.as_slice())
                .map_err(|_| ReceiverError::DeserializeVAAFailed)?;

        msg!(
            "There are {} attestations in this batch.",
            batch.price_attestations.len()
        );

        for attestation in batch.price_attestations {
            msg!("product_id: {}", attestation.product_id);
            msg!("price_id: {}", attestation.price_id);
            msg!("price: {}", attestation.price);
            msg!("conf: {}", attestation.conf);
            msg!("ema_price: {}", attestation.ema_price);
            msg!("ema_conf: {}", attestation.ema_conf);
            msg!("num_publishers: {}", attestation.num_publishers);
            msg!("publish_time: {}", attestation.publish_time);
            msg!("attestation_time: {}", attestation.attestation_time);
        }

        Ok(())
    }

    pub fn update(
        _ctx: Context<Update>,
        data: Vec<u8>,
        recovery_id: u8,
        signature: [u8; 64],
    ) -> Result<()> {
        msg!("udpate");
        // This costs about 10k compute units
        let message_hash = {
            let mut hasher = keccak::Hasher::default();
            hasher.hash(&data);
            hasher.result()
        };

        // This costs about 25k compute units
        let recovered_pubkey = secp256k1_recover(&message_hash.0, recovery_id, &signature)
            .map_err(|_| ProgramError::InvalidArgument)?;

        msg!(
            "Recovered key: {}",
            recovered_pubkey.0.encode_hex::<String>()
        );

        // TODO: Check the pubkey is an expected value.
        // Here we are checking the secp256k1 pubkey against a known authorized pubkey.
        //
        // if recovered_pubkey.0 != AUTHORIZED_PUBLIC_KEY {
        //  return Err(ProgramError::InvalidArgument);
        // }

        Ok(())
    }

    /// Verifies the accumulator update data header then invokes a CPI call to wormhole::postVAA
    ///
    /// * `data` - Bytes of the AccumulatorUpdateData response from hermes with the updates omitted
    ///           (i.e. the `updates` field is an empty array). The updates are removed so that
    ///           all the data needed for postVaa can fit in one txn.
    pub fn post_accumulator_update_vaa(
        ctx: Context<PostAccUpdateDataVaa>,
        data: Vec<u8>, // accumulatorUpdateData {vaa, updates: [] }
    ) -> Result<()> {
        // verify accumulator update data header
        let accumulator_update_data = AccumulatorUpdateData::try_from_slice(data.as_slice())
            .map_err(|_| ProgramError::InvalidArgument)?;
        match accumulator_update_data.proof {
            Proof::WormholeMerkle { vaa, updates: _ } => {
                let vaa: Vaa<&RawMessage> = serde_wormhole::from_slice(vaa.as_ref()).unwrap();
                let (header, body): (Header, Body<&RawMessage>) = vaa.into();

                let post_vaa_ix_data = PostVAAInstructionData {
                    version:            header.version,
                    guardian_set_index: header.guardian_set_index,
                    timestamp:          body.timestamp,
                    nonce:              body.nonce,
                    emitter_chain:      body.emitter_chain.into(),
                    emitter_address:    body.emitter_address.0,
                    sequence:           body.sequence,
                    consistency_level:  body.consistency_level,
                    payload:            body.payload.to_vec(),
                };
                let post_vaa_ix = Instruction {
                    program_id: ctx.accounts.wormhole_program.key(),
                    accounts:   vec![
                        AccountMeta::new_readonly(ctx.accounts.guardian_set.key(), false),
                        AccountMeta::new_readonly(ctx.accounts.bridge_config.key(), false),
                        AccountMeta::new_readonly(ctx.accounts.signature_set.key(), false),
                        AccountMeta::new(ctx.accounts.vaa.key(), false),
                        AccountMeta::new(ctx.accounts.payer.key(), true),
                        AccountMeta::new_readonly(Clock::id(), false),
                        AccountMeta::new_readonly(Rent::id(), false),
                        AccountMeta::new_readonly(ctx.accounts.system_program.key(), false),
                    ],
                    data:       (POST_VAA, post_vaa_ix_data).try_to_vec()?,
                };
                invoke(&post_vaa_ix, ctx.accounts.to_account_infos().as_slice())?;
            }
        }
        Ok(())
    }

    #[allow(unused_variables)]
    pub fn post_updates(
        ctx: Context<PostUpdates>,
        vaa_hash: [u8; 32], // used for pda seeds
        price_updates: Vec<Vec<u8>>,
    ) -> Result<()> {
        let vaa = &ctx.accounts.posted_vaa; // let posted_vaa_data = PostedVaaData::try_deserialize_unchecked(&mut &**vaa.try_borrow_data()?)?;
        let wh_message = WormholeMessage::try_from_bytes(vaa.payload.as_slice())
            .map_err(|_| ReceiverError::InvalidWormholeMessage)?;
        msg!("constructed wh_message {:?}", wh_message);
        let root: MerkleRoot<Keccak160> = MerkleRoot::new(match wh_message.payload {
            WormholePayload::Merkle(merkle_root) => merkle_root.root,
        });

        let mut count_updates = 0;

        let price_updates_len = price_updates.len();
        for price_update in price_updates {
            let merkle_price_update =
                from_slice::<byteorder::BE, MerklePriceUpdate>(price_update.as_slice())
                    .map_err(|_| ReceiverError::DeserializeUpdateFailed)?;
            let message_vec = Vec::from(merkle_price_update.message);
            if !root.check(merkle_price_update.proof, &message_vec) {
                return err!(ReceiverError::InvalidPriceUpdate);
            }
            let msg = from_slice::<byteorder::BE, Message>(&message_vec)
                .map_err(|_| ReceiverError::InvalidAccumulatorMessage)?;

            match msg {
                Message::PriceFeedMessage(price_feed_message) => {
                    count_updates += 1;
                    msg!("price_feed_message: {:?}", price_feed_message);
                }
                Message::TwapMessage(twap_message) => {
                    count_updates += 1;
                    msg!("twap_message: {:?}", twap_message);
                }
                _ => return err!(ReceiverError::InvalidAccumulatorMessageType),
            }
        }
        msg!("verified {} / {} updates", count_updates, price_updates_len);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct DecodePostedVaa<'info> {
    #[account(mut)]
    pub payer:      Signer<'info>,
    #[account(
        constraint = (Chain::from(posted_vaa.emitter_chain()) == Solana || Chain::from(posted_vaa.emitter_chain()) == Pythnet) @ ReceiverError::EmitterChainNotSolanaOrPythnet,
    )]
    pub posted_vaa: Account<'info, AnchorVaa>,
}

impl crate::accounts::DecodePostedVaa {
    pub fn populate(payer: &Pubkey, posted_vaa: &Pubkey) -> Self {
        crate::accounts::DecodePostedVaa {
            payer:      *payer,
            posted_vaa: *posted_vaa,
        }
    }
}

#[derive(Accounts)]
pub struct Update<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(vaa_hash: [u8; 32])]
pub struct PostUpdates<'info> {
    #[account(mut)]
    pub payer:      Signer<'info>,
    #[account(
        constraint = Chain::from(posted_vaa.emitter_chain()) == Pythnet @ ReceiverError::EmitterChainNotSolanaOrPythnet,
        seeds = [
            SEED_PREFIX_POSTED_VAA,
            &vaa_hash
        ],
        seeds::program = wormhole_anchor::program::Wormhole::id(),
        bump
    )]
    pub posted_vaa: Box<Account<'info, AnchorVaa>>,
}

impl crate::accounts::PostUpdates {
    pub fn populate(payer: &Pubkey, posted_vaa: &Pubkey) -> Self {
        crate::accounts::PostUpdates {
            payer:      *payer,
            posted_vaa: *posted_vaa,
        }
    }
}

#[derive(Accounts)]
pub struct PostAccUpdateDataVaa<'info> {
    // wormhole postVaa accounts
    /// CHECK: guardian set
    pub guardian_set:     AccountInfo<'info>,
    /// CHECK: bridge config
    pub bridge_config:    AccountInfo<'info>,
    /// CHECK: signature set.
    pub signature_set:    AccountInfo<'info>,
    /// CHECK: posted vaa data
    #[account(mut)]
    pub vaa:              AccountInfo<'info>,
    #[account(mut)]
    pub payer:            Signer<'info>,
    pub clock:            Sysvar<'info, Clock>,
    pub rent:             Sysvar<'info, Rent>,
    pub system_program:   Program<'info, System>,
    pub wormhole_program: Program<'info, wormhole_anchor::program::Wormhole>,
}

#[derive(Debug, Eq, PartialEq, AnchorSerialize, AnchorDeserialize)]
pub struct PostVAAInstructionData {
    // Header part
    pub version:            u8,
    pub guardian_set_index: u32,

    // Body part
    pub timestamp:         u32,
    pub nonce:             u32,
    pub emitter_chain:     u16,
    pub emitter_address:   [u8; 32],
    pub sequence:          u64,
    pub consistency_level: u8,
    pub payload:           Vec<u8>,
}

#[derive(Default, AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub struct GuardianSet {
    /// Index representing an incrementing version number for this guardian set.
    pub index:           u32,
    /// ETH style public keys
    pub keys:            Vec<[u8; 20]>,
    /// Timestamp representing the time this guardian became active.
    pub creation_time:   u32,
    /// Expiration time when VAAs issued by this set are no longer valid.
    pub expiration_time: u32,
}

impl AccountDeserialize for GuardianSet {
    fn try_deserialize_unchecked(buf: &mut &[u8]) -> Result<Self> {
        Self::deserialize(buf).map_err(Into::into)
    }
}

impl AccountSerialize for GuardianSet {
}

impl Owner for GuardianSet {
    fn owner() -> Pubkey {
        wormhole_anchor::program::Wormhole::id()
    }
}