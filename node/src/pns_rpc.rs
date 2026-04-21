use std::sync::Arc;

use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use pns_types::{DomainHash, ListingInfo, NameRecord};
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use paseo_runtime::{AccountId, Balance, opaque::Block};
use pns_runtime_api::PnsStorageApi as PnsRuntimeApi;

#[rpc(server)]
pub trait PnsStorageApi {
    #[method(name = "pns_getInfo")]
    fn get_info(&self, node: DomainHash) -> RpcResult<Option<NameRecord<AccountId, u64, Balance>>>;

    #[method(name = "pns_resolveName")]
    fn resolve_name(&self, name: String) -> RpcResult<Option<NameRecord<AccountId, u64, Balance>>>;

    #[method(name = "pns_lookup")]
    fn lookup(&self, node: DomainHash, record_types: Vec<u32>) -> RpcResult<Vec<(u32, Vec<u8>)>>;

    #[method(name = "pns_getListing")]
    fn get_listing(&self, name: String) -> RpcResult<Option<ListingInfo<AccountId, Balance, u64>>>;

    #[method(name = "pns_lookupByName")]
    fn lookup_by_name(&self, name: String, record_types: Vec<u32>) -> RpcResult<Vec<(u32, Vec<u8>)>>;

}

pub struct PnsRpc<C> {
    client: Arc<C>,
}

impl<C> PnsRpc<C> {
    pub fn new(client: Arc<C>) -> Self {
        Self { client }
    }
}

impl<C> PnsStorageApiServer for PnsRpc<C>
where
    C: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync + 'static,
    C::Api: pns_runtime_api::PnsStorageApi<Block, u64, Balance, AccountId>,
{
    fn get_info(&self, node: DomainHash) -> RpcResult<Option<NameRecord<AccountId, u64, Balance>>> {
        let chain = self.client.info();
        let api = self.client.runtime_api();
        api.get_info(chain.best_hash, node)
            .map_err(|e| jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INTERNAL_ERROR_CODE, "Runtime error", Some(format!("{:?}", e)),
            ))
            .map(|opt| opt.map(|mut r| { r.read_block_hash = chain.best_hash; r.read_block_number = chain.best_number; r }))
    }

    fn resolve_name(&self, name: String) -> RpcResult<Option<NameRecord<AccountId, u64, Balance>>> {
        if name.len() > 128 {
            return Err(jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE, "name too long", None::<()>,
            ));
        }
        let chain = self.client.info();
        let api = self.client.runtime_api();
        api.resolve_name(chain.best_hash, name.into_bytes())
            .map_err(|e| jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INTERNAL_ERROR_CODE, "Runtime error", Some(format!("{:?}", e)),
            ))
            .map(|opt| opt.map(|mut r| { r.read_block_hash = chain.best_hash; r.read_block_number = chain.best_number; r }))
    }

    fn lookup(&self, node: DomainHash, record_types: Vec<u32>) -> RpcResult<Vec<(u32, Vec<u8>)>> {
        if record_types.len() > 50 {
            return Err(jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE, "too many record types (max 50)", None::<()>,
            ));
        }
        let api = self.client.runtime_api();
        let best = self.client.info().best_hash;
        let pns_types = record_types.into_iter()
            .map(|code| hickory_proto::rr::RecordType::from(code as u16).into())
            .collect();
        let records = api.lookup(best, node, pns_types).map_err(|e| jsonrpsee::types::ErrorObject::owned(
            jsonrpsee::types::error::INTERNAL_ERROR_CODE, "Runtime error", Some(format!("{:?}", e)),
        ))?;
        Ok(records.into_iter().map(|(rt, data)| {
            let code = u16::from(hickory_proto::rr::RecordType::from(rt)) as u32;
            (code, data)
        }).collect())
    }

    fn get_listing(&self, name: String) -> RpcResult<Option<ListingInfo<AccountId, Balance, u64>>> {
        if name.len() > 128 {
            return Err(jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE, "name too long", None::<()>,
            ));
        }
        let chain = self.client.info();
        let api = self.client.runtime_api();
        api.get_listing(chain.best_hash, name.into_bytes())
            .map_err(|e| jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INTERNAL_ERROR_CODE, "Runtime error", Some(format!("{:?}", e)),
            ))
            .map(|opt| opt.map(|mut r| { r.read_block_hash = chain.best_hash; r.read_block_number = chain.best_number; r }))
    }

    fn lookup_by_name(&self, name: String, record_types: Vec<u32>) -> RpcResult<Vec<(u32, Vec<u8>)>> {
        if name.len() > 128 {
            return Err(jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE, "name too long", None::<()>,
            ));
        }
        if record_types.len() > 50 {
            return Err(jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE, "too many record types (max 50)", None::<()>,
            ));
        }
        let api = self.client.runtime_api();
        let best = self.client.info().best_hash;
        let pns_types = record_types.into_iter()
            .map(|code| hickory_proto::rr::RecordType::from(code as u16).into())
            .collect();
        let records = api.lookup_by_name(best, name.into_bytes(), pns_types)
            .map_err(|e| jsonrpsee::types::ErrorObject::owned(
                jsonrpsee::types::error::INTERNAL_ERROR_CODE, "Runtime error", Some(format!("{:?}", e)),
            ))?;
        Ok(records.into_iter().map(|(rt, data)| {
            let code = u16::from(hickory_proto::rr::RecordType::from(rt)) as u32;
            (code, data)
        }).collect())
    }

}
