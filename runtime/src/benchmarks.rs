// Benchmarks list — consumed by the `list_benchmarks!` / `add_benchmarks!`
// macro calls in `apis.rs` under the `runtime-benchmarks` feature.
//
// Each row pairs a pallet module path with the construct_runtime! alias.
// Phase 1 wires PnsPriceOracle only; subsequent phases add more PNS
// pallets as their benchmark functions land.

#[cfg(feature = "runtime-benchmarks")]
frame_benchmarking::define_benchmarks!(
    [frame_benchmarking, BaselineBench::<Runtime>]
    [frame_system, SystemBench::<Runtime>]
    [frame_system_extensions, SystemExtensionsBench::<Runtime>]
    [pallet_balances, Balances]
    [pallet_timestamp, Timestamp]
    [pns_registrar::price_oracle, PnsPriceOracle]
    [pns_registrar::registry, PnsRegistry]
    [pns_registrar::registrar, PnsRegistrar]
    [pns_resolvers::resolvers, PnsResolvers]
    [pns_marketplace, PnsMarketplace]
    [zk_pki_pallet, ZkPki]
);
