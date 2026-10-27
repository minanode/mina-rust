---
title: send
description: Send a payment transaction to the network
sidebar_position: 4
---

# send

Send a payment transaction to the network.

## Basic usage

```bash
mina wallet send \
  --from /path/to/encrypted/key \
  --to <receiver_public_key> \
  --amount <amount_in_mina> \
  --fee <fee_in_mina>
```

## Arguments

**Required:**

- `--from <PATH>` - Path to encrypted sender key file
- `--to <PUBLIC_KEY>` - Receiver's public key (Base58Check encoded)
- `--amount <MINA>` - Amount in MINA, like `2.5` (at most 9 decimal places)
- `--fee <MINA>` - Transaction fee in MINA, like `0.1`

**Optional:**

- `--memo <MEMO>` - Transaction memo (max 32 bytes, default: empty)
- `--nonce <NONCE>` - Transaction nonce (default: fetched from node)
- `--valid-until <SLOT>` - Slot until which transaction is valid (default: never
  expires)
- `--network <NETWORK>` - Network for signing: `mainnet` or `devnet` (default:
  `devnet`)
- `--node <URL>` - The node's HTTP base URL (or `MINA_NODE_URL`; default:
  `http://localhost:3000`)

**Environment:**

- `MINA_PRIVKEY_PASS` - Password to decrypt the sender key. The password is read
  only from this variable. A password given as an argument is refused.

## Examples

### Send payment on devnet

```bash
mina wallet send \
  --from ./keys/my-wallet \
  --to B62qre3erTHfzQckNuibViWQGyyKwZseztqrjPZBv6SQF384Rg6ESAy \
  --amount 1 \
  --fee 0.01 \
  --network devnet
```

### Send payment on mainnet with memo

```bash
mina wallet send \
  --from ./keys/my-wallet \
  --to B62qre3erTHfzQckNuibViWQGyyKwZseztqrjPZBv6SQF384Rg6ESAy \
  --amount 5 \
  --fee 0.01 \
  --memo "Payment for services" \
  --network mainnet
```

### Send payment to remote node

```bash
mina wallet send \
  --from ./keys/my-wallet \
  --to B62qre3erTHfzQckNuibViWQGyyKwZseztqrjPZBv6SQF384Rg6ESAy \
  --amount 1 \
  --fee 0.01 \
  --node https://node.example.com:3000
```

### Set the password

```bash
read -rs -p "Key password: " MINA_PRIVKEY_PASS && export MINA_PRIVKEY_PASS

mina wallet send \
  --from ./keys/my-wallet \
  --to B62qre3erTHfzQckNuibViWQGyyKwZseztqrjPZBv6SQF384Rg6ESAy \
  --amount 1 \
  --fee 0.01
```

## How it works

When you send a payment, the CLI:

1. **Decrypts your key** - Uses the provided password to decrypt your private
   key
2. **Fetches nonce** - Queries the node via GraphQL to get your current account
   nonce (if not specified)
3. **Creates payload** - Builds the payment transaction payload with all details
4. **Signs transaction** - Signs the transaction using your private key and the
   correct network ID
5. **Submits to node** - Sends the signed transaction to the node via GraphQL
   `sendPayment` mutation
6. **Returns hash** - Displays the transaction hash, which can be used with
   `mina wallet status` to track the transaction

## Network selection

The `--network` flag controls which network the transaction is signed for:

- `devnet` - For development and testing (default)
- `mainnet` - For production transactions

**Important:** Make sure to use the correct network flag. A transaction signed
for devnet will not be valid on mainnet and vice versa.
