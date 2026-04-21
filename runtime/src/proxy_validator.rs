//! Paseo-runtime implementation of `zk_pki_primitives::proxy::ValidateProxy`,
//! reading `pallet_proxy::Proxies` directly.
//!
//! This is the production variant; a proxy relationship is considered to
//! exist iff the delegator has a `ProxyDefinition` in
//! `pallet_proxy::Proxies::<T>` whose `delegate` matches the delegatee.
//! Used to enforce invariant #12 on `register_root` / `issue_issuer_cert`.

use core::marker::PhantomData;
use zk_pki_primitives::proxy::ValidateProxy;

pub struct PalletProxyValidator<T>(PhantomData<T>);

impl<T> ValidateProxy<T::AccountId> for PalletProxyValidator<T>
where
    T: pallet_proxy::Config,
{
    fn has_proxy(delegator: &T::AccountId, delegatee: &T::AccountId) -> bool {
        let (proxies, _deposit) = pallet_proxy::Proxies::<T>::get(delegator);
        proxies.iter().any(|def| &def.delegate == delegatee)
    }
}
