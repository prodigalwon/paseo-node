//! Merged RPC extensions: pns_ + zkPki_ + system + txPayment.

#![warn(missing_docs)]

use std::sync::Arc;

use jsonrpsee::RpcModule;
use sc_transaction_pool_api::TransactionPool;
use paseo_runtime::{opaque::Block, AccountId, Balance, Nonce};
use sp_api::ProvideRuntimeApi;
use sp_block_builder::BlockBuilder;
use sp_blockchain::{Error as BlockChainError, HeaderBackend, HeaderMetadata};

use crate::pns_rpc::{PnsRpc, PnsStorageApiServer};
use zk_pki_rpc::ZkPkiRpcApiServer;

/// Full client dependencies.
pub struct FullDeps<C, P> {
	/// The client instance to use.
	pub client: Arc<C>,
	/// Transaction pool instance.
	pub pool: Arc<P>,
}

/// Instantiate all full RPC extensions.
pub fn create_full<C, P>(
	deps: FullDeps<C, P>,
) -> Result<RpcModule<()>, Box<dyn std::error::Error + Send + Sync>>
where
	C: ProvideRuntimeApi<Block>,
	C: HeaderBackend<Block> + HeaderMetadata<Block, Error = BlockChainError> + 'static,
	C: Send + Sync + 'static,
	C::Api: substrate_frame_rpc_system::AccountNonceApi<Block, AccountId, Nonce>,
	C::Api: pallet_transaction_payment_rpc::TransactionPaymentRuntimeApi<Block, Balance>,
	C::Api: pns_runtime_api::PnsStorageApi<Block, u64, Balance, AccountId>,
	// ZkPkiApi generics collapsed in the TODO-5 / optimization pass:
	// the runtime API is now `<Block, AccountId>` only; block numbers
	// surface as `u64` internally via `UniqueSaturatedInto`.
	C::Api: zk_pki_primitives::runtime_api::ZkPkiApi<Block, AccountId>,
	C::Api: secret_squirrel_runtime_api::SecretSquirrelApi<Block>,
	C::Api: BlockBuilder<Block>,
	P: TransactionPool + 'static,
{
	use pallet_transaction_payment_rpc::{TransactionPayment, TransactionPaymentApiServer};
	use substrate_frame_rpc_system::{System, SystemApiServer};

	let mut module = RpcModule::new(());
	let FullDeps { client, pool } = deps;

	module.merge(System::new(client.clone(), pool).into_rpc())?;
	module.merge(TransactionPayment::new(client.clone()).into_rpc())?;

	// PNS RPC (pns_ namespace)
	module.merge(PnsRpc::new(client.clone()).into_rpc())?;

	// ZK-PKI RPC (zkPki_ namespace)
	module.merge(zk_pki_rpc::ZkPkiRpc::<_, Block>::new(client.clone()).into_rpc())?;

	// SecretSquirrel has no dedicated RPC crate yet — its runtime API is
	// accessible via the generic `state_call` RPC. When a dedicated RPC
	// crate is added, wire it here.

	Ok(module)
}
