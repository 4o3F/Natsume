#!/usr/bin/env bash
# Synthetic-only PTY/envelope integration check; requires expect and openssl.
set -euo pipefail
helper="$1"
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
mkdir "$work/bin"
cat > "$work/bin/systemd-run" <<'STUB'
#!/bin/bash
set -euo pipefail
printf 'Login name: '
read -r login
test "$login" = synthetic-admin
printf 'Password: '
read -rs password
printf 'Confirm password: '
read -rs confirmation
test "$password" = Synthetic-PTY-Fixture
test "$confirmation" = "$password"
STUB
chmod 700 "$work/bin/systemd-run"
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$work/key.pem" 2>/dev/null
openssl pkey -in "$work/key.pem" -pubout > "$work/public.pem"
printf Synthetic-PTY-Fixture | openssl pkeyutl -encrypt -pubin -inkey "$work/public.pem" -pkeyopt rsa_padding_mode:oaep > "$work/envelope"
PATH="$work/bin:$PATH" expect "$helper" "$work" v2.10.3 < "$work/envelope"
printf 'Synthetic PTY bootstrap fixture passed\n'
