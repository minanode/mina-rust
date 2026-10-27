---
title: generate
description: Generate a new encrypted key pair
sidebar_position: 3
---

# generate

Generate a new encrypted key pair for use with Mina.

## Basic usage

```bash
mina wallet generate --output /path/to/key
```

## Arguments

**Required:**

- `--output <PATH>` - Path where the encrypted key file will be saved

**Optional:**

- `-s, --secret-key <KEY>` - Encrypt this secret key instead of a new one (or
  `MINA_SEC_KEY`)

**Environment:**

- `MINA_PRIVKEY_PASS` - Password to encrypt the key, required and non-empty. The
  password is read only from this variable. A password given as an argument is
  refused.

## Example

```bash
read -rs -p "Key password: " MINA_PRIVKEY_PASS && export MINA_PRIVKEY_PASS
mina wallet generate --output ./keys/my-new-wallet
```

This command generates a new random keypair, encrypts the private key with the
provided password, and saves it to the specified path. It also creates a `.pub`
file containing the public key.
