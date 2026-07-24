# ADR-0002: Library-first Machine Hardware ID

Use sysinfo Product/Motherboard first. Supplement with smbios-lib, actual raw-cpuid processor serial, procfs MountInfo, udev and libsystemd. Raw serials stay in the privileged helper. The shared crate is pure and owns only normalized evidence, fingerprints, deterministic UUIDv5 candidates and startup comparison.

The UUIDv5 namespace is a public, immutable site `fleet_namespace_uuid` injected through signed deployment material and preserved across contest resets. There is no alias graph, machine-ID revision or installation instance.
