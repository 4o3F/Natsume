#!/bin/bash

# ============================================
# Certificate Issuance Script - Issue ECC certificate for a domain using CA
# ============================================

# Please modify the following variables to your actual values
CA_CERT="/path/to/your/ca.crt"        # CA certificate file path
CA_KEY="/path/to/your/ca.key"         # CA private key file path
DOMAIN="example.com"                   # Primary domain name
DAYS=365                               # Certificate validity period in days
COUNTRY="CN"                           # Country code (optional)
STATE="Beijing"                        # State/Province (optional)
CITY="Beijing"                         # City (optional)
ORGANIZATION="My Organization"         # Organization name (optional)
ECC_CURVE="prime256v1"                  # ECC curve: prime256v1 or secp384r1

# ============================================
# Script starts - No need to modify below
# ============================================

# Check required files
if [ ! -f "$CA_CERT" ]; then
    echo "Error: CA certificate file does not exist: $CA_CERT"
    exit 1
fi

if [ ! -f "$CA_KEY" ]; then
    echo "Error: CA private key file does not exist: $CA_KEY"
    exit 1
fi

# Set filenames
DOMAIN_KEY="${DOMAIN}.key"
DOMAIN_CSR="${DOMAIN}.csr"
DOMAIN_CRT="${DOMAIN}.crt"
DOMAIN_EXT="${DOMAIN}.ext"

echo "========================================="
echo "Starting certificate generation for domain: $DOMAIN"
echo "CA Certificate: $CA_CERT"
echo "Validity: $DAYS days"
echo "ECC Curve: $ECC_CURVE"
echo "========================================="

# 1. Generate domain private key
echo "[1/4] Generating domain private key..."
openssl ecparam -genkey -name $ECC_CURVE -out $DOMAIN_KEY
if [ $? -ne 0 ]; then
    echo "Error: Private key generation failed"
    exit 1
fi
echo "✓ Private key generated: $DOMAIN_KEY"

# 2. Create extension configuration file
echo "[2/4] Creating certificate extension configuration..."
cat > $DOMAIN_EXT << EOF
subjectAltName = DNS:$DOMAIN, DNS:www.$DOMAIN
keyUsage = digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
basicConstraints = CA:FALSE
EOF
echo "✓ Extension configuration created: $DOMAIN_EXT"

# 3. Generate CSR
echo "[3/4] Generating Certificate Signing Request..."
openssl req -new -key $DOMAIN_KEY \
    -out $DOMAIN_CSR \
    -subj "/C=$COUNTRY/ST=$STATE/L=$CITY/O=$ORGANIZATION/CN=$DOMAIN"
if [ $? -ne 0 ]; then
    echo "Error: CSR generation failed"
    exit 1
fi
echo "✓ CSR generated: $DOMAIN_CSR"

# 4. Issue certificate using CA
echo "[4/4] Issuing certificate using CA..."
openssl x509 -req -in $DOMAIN_CSR \
    -CA $CA_CERT -CAkey $CA_KEY \
    -CAcreateserial -out $DOMAIN_CRT \
    -days $DAYS -sha256 \
    -extfile $DOMAIN_EXT
if [ $? -ne 0 ]; then
    echo "Error: Certificate issuance failed"
    exit 1
fi
echo "✓ Certificate issued: $DOMAIN_CRT"

# 5. Verify certificate
echo "========================================="
echo "Verifying generated certificate..."
openssl verify -CAfile $CA_CERT $DOMAIN_CRT
if [ $? -eq 0 ]; then
    echo "✓ Certificate verification successful"
else
    echo "Warning: Certificate verification failed, please check"
fi

# 6. Display certificate information
echo "========================================="
echo "Certificate information summary:"
openssl x509 -in $DOMAIN_CRT -noout -subject -issuer -dates
echo "========================================="
echo "All generated files:"
ls -la $DOMAIN_KEY $DOMAIN_CSR $DOMAIN_CRT $DOMAIN_EXT
echo "========================================="
echo "Done!"