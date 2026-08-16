# Device daemon test fixtures

- `gateway-csr.der.base64` — a public PKCS#10 CSR (base64 of DER) pinning the raw-SPKI
  extraction and SHA-256 derivation across implementations. It contains only a public
  key and a signature.

Never commit real passwords, root keys, private keys or raw hardware serials.
