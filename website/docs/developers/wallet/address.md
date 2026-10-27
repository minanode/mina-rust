---
title: address
description: Get the public address from an encrypted key file
sidebar_position: 1
---

# address

Get the public address from an encrypted key file.

## Basic usage

```bash
mina wallet address --from /path/to/encrypted/key
```

## Arguments

**Required:**

- `--from <PATH>` - Path to encrypted key file

**Environment:**

- `MINA_PRIVKEY_PASS` - Password to decrypt the key. The password is read only
  from this variable. A password given as an argument is refused.

## Example

```bash
read -rs -p "Key password: " MINA_PRIVKEY_PASS && export MINA_PRIVKEY_PASS
mina wallet address --from ./keys/my-wallet
```

This command simply decrypts the key file and displays the associated public
address. It does not require a connection to a node.
