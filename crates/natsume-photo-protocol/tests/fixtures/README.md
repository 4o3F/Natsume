# Result signature vector

`results-v1.manifest.json` is the canonical payload. The signing input is
`b"Natsume/Photo/team-results/v1\0"` followed by its exact UTF-8 bytes.
`results-v1.signature.hex` is the lowercase Ed25519 signature;
`results-v1.signed.json` is the complete canonical JSON envelope.

These files were generated independently using Python `cryptography` 46.0.5 and
the public TEST 1 seed in [RFC 8032 §7.1](https://www.rfc-editor.org/rfc/rfc8032.html#section-7.1).
The test seed is not an application credential. All fixture property names are
ASCII and numeric values are exact small integers, so Python's sorted compact
JSON matches JCS **for this fixture**. It is not a general RFC 8785 serializer.
The Rust tests separately cover RFC property-order and number vectors.

From this directory, the frozen vector can be independently checked with:

```python
import json
from pathlib import Path
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

key = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(
    "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
))
payload = Path("results-v1.manifest.json").read_bytes()
canonical = json.dumps(json.loads(payload), ensure_ascii=False,
                       sort_keys=True, separators=(",", ":")).encode()
assert payload == canonical
signature = key.sign(b"Natsume/Photo/team-results/v1\0" + payload)
assert signature.hex() == Path("results-v1.signature.hex").read_text()
key.public_key().verify(signature, b"Natsume/Photo/team-results/v1\0" + payload)
envelope = {"manifest": json.loads(payload), "signature": signature.hex()}
assert json.dumps(envelope, ensure_ascii=False, sort_keys=True,
                  separators=(",", ":")).encode() == Path("results-v1.signed.json").read_bytes()
```
