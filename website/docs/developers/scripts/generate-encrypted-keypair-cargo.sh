#!/bin/bash

set -eou pipefail

# This creates `web-node-key.zip` containing:
#
# - `producer-key` (encrypted private key)
# - `producer-key.pub` (public key)
# - `producer-key-password` (password file)

# Create temporary working directory
mkdir producer-key-tmp

# Create password file
echo "mypassword" > producer-key-tmp/producer-key-password

# Generate encrypted key pair. The password is read from MINA_PRIVKEY_PASS:
# the command refuses a password given as an argument.
MINA_PRIVKEY_PASS="$(cat producer-key-tmp/producer-key-password)" \
  cargo run -r --bin mina -- wallet generate \
    --output producer-key-tmp/producer-key

# Package into ZIP for upload through the web interface and clean up intermediates
cd producer-key-tmp
zip web-node-key.zip producer-key producer-key.pub producer-key-password
mv web-node-key.zip ../
cd ../
rm -rf producer-key-tmp
