---
title: Introduction
description: CLI wallet commands for managing accounts and sending transactions
sidebar_position: 15
---

# Wallet operations

The Mina CLI provides wallet functionality for sending transactions and managing
accounts. All wallet operations use encrypted key files and interact with a
running Mina node via GraphQL.

## Introduction

The wallet commands provide a complete set of tools for managing Mina accounts
and sending transactions directly from the command line. These commands handle
key generation, account queries, and transaction submission.

### Prerequisites

Before using wallet commands, you need:

- An encrypted private key file
- The password to decrypt the key
- A running Mina node (local or remote) - only required for sending transactions
  and checking balances

### Environment variables

Key passwords are read only from `MINA_PRIVKEY_PASS`. A password given as an
argument is refused. You can set these environment variables once per shell:

```bash
read -rs -p "Key password: " MINA_PRIVKEY_PASS && export MINA_PRIVKEY_PASS
export MINA_NETWORK="mainnet"  # or "devnet"
```

## Commands

The wallet CLI provides the following commands, each documented in detail in
their respective sections:

- **[address](./address.md)** - Get the public address from an encrypted key
  file
- **[balance](./balance.md)** - Query account balance and details using GraphQL
- **[generate](./generate.md)** - Generate a new encrypted key pair
- **[send](./send.md)** - Send a payment transaction to the network
- **[status](./status.md)** - Check the status of a submitted transaction

## Understanding amounts

Amounts and fees on the command line are in **MINA**, with at most 9 decimal
places:

- `--amount 1` sends 1 MINA (1,000,000,000 nanomina)
- `--fee 0.1` pays a 0.1 MINA fee
- `--fee 0.01` pays a 0.01 MINA fee (common minimum fee)

## GraphQL integration

The wallet commands use the node's GraphQL API:

- **Account query** - Fetches current nonce and account information (`balance`
  command)
- **sendPayment mutation** - Submits signed transactions to the network (`send`
  command)
- **transactionStatus query** - Checks if a transaction is included in the
  blockchain (`status` command)
- **pooledUserCommands query** - Lists pending transactions in the mempool
  (`status` command)

For more details on the GraphQL API, see the
[GraphQL API](../api-and-data/graphql-api.mdx) documentation.
