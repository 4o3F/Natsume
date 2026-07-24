# ADR-0003: Direct nFPM package composition

nFPM maps final build outputs and package-owned rootfs directly into `natsume-server` and `natsume-client`. Do not maintain a duplicate staging tree. Migrate to debhelper by ADR if Debian policy requirements outgrow nFPM.
