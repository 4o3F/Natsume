# Hardware identity fixtures

The implementation repository stores anonymized fixtures for:

- valid and placeholder SMBIOS fields;
- duplicate Product UUIDs;
- missing board/chassis/processor serials;
- CPUID Processor Serial present and absent;
- root on SATA, NVMe, LVM, dm-crypt and overlay/container mounts;
- udev parent ambiguity;
- complete contradictory hardware evidence after a configured-disk copy;
- temporarily unavailable evidence that must not trigger deletion;
- fleet collision rejection.

App-local IDs and `/etc/machine-id` are not identity fallbacks because they can be copied with the system disk. Raw production serials must never be committed.
