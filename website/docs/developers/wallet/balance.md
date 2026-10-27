---
title: balance
description: Query account balance and details using GraphQL
sidebar_position: 2
---

# balance

Query the balance and details of an account using GraphQL.

## Basic usage

```bash
# Check balance using key file
mina wallet balance --from /path/to/encrypted/key

# Check balance using public address
mina wallet balance --address <PUBLIC_KEY>
```

## Arguments

**Required (one of):**

- `--from <PATH>` - Path to encrypted key file
- `--address <PUBLIC_KEY>` - Public key to query directly

**Optional:**

- `--node <URL>` - The node's HTTP base URL (or `MINA_NODE_URL`; default:
  `http://localhost:3000`)
- `--format <FORMAT>` - Output format: `text` (default) or `json`

**Environment:**

- `MINA_PRIVKEY_PASS` - Password to decrypt the key, required with `--from`. The
  password is read only from this variable. A password given as an argument is
  refused.

## Examples

### Check balance using key file

```bash
read -rs -p "Key password: " MINA_PRIVKEY_PASS && export MINA_PRIVKEY_PASS
mina wallet balance --from ./keys/my-wallet
```

### Check balance using public address

```bash
mina wallet balance \
  --address B62qre3erTHfzQckNuibViWQGyyKwZseztqrjPZBv6SQF384Rg6ESAy
```

### Check balance on remote node

```bash
mina wallet balance \
  --address B62qre3erTHfzQckNuibViWQGyyKwZseztqrjPZBv6SQF384Rg6ESAy \
  --node https://node.example.com:3000
```

### Get balance in JSON format

```bash
mina wallet balance \
  --address B62qre3erTHfzQckNuibViWQGyyKwZseztqrjPZBv6SQF384Rg6ESAy \
  --format json
```

## Output

The balance command displays:

- **Total balance** - Total amount of MINA in the account (both nanomina and
  MINA)
- **Liquid balance** - Amount available for spending
- **Locked balance** - Amount locked due to vesting schedule
- **Nonce** - Current account nonce
- **Delegate** - Public key of the delegate (if set)

### Text format (default)

```
Account: B62qre3erTHfzQckNuibViWQGyyKwZseztqrjPZBv6SQF384Rg6ESAy

Balance:
  Total:  1000.000000000 MINA
  Liquid: 800.000000000 MINA
  Locked: 200.000000000 MINA

Nonce: 5

Delegate: B62qkfHpLpELqpMK6ZvUTJ5wRqKDRF3UHyJ4Kv3FU79Sgs4qpBnx5RG
```

### JSON format

```json
{
  "account": "B62qre3erTHfzQckNuibViWQGyyKwZseztqrjPZBv6SQF384Rg6ESAy",
  "balance": {
    "total": "1000000000000",
    "total_mina": "1000.000000000",
    "liquid": "800000000000",
    "liquid_mina": "800.000000000",
    "locked": "200000000000",
    "locked_mina": "200.000000000"
  },
  "nonce": "5",
  "delegate": "B62qkfHpLpELqpMK6ZvUTJ5wRqKDRF3UHyJ4Kv3FU79Sgs4qpBnx5RG"
}
```

The JSON format includes both nanomina (raw values) and formatted MINA values
for convenience.
