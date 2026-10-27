# Archive end-to-end scripts

Our node feeding OCaml 4.0.2's archive process and a user-level Postgres, as
R43b.1 ran it. No sudo, no docker.

## Install (once, user-level)

```bash
# Postgres 16 from Ubuntu noble, extracted.
mkdir -p /tmp/pgdl && cd /tmp/pgdl
apt-get download postgresql-16 postgresql-client-16 postgresql-common \
  postgresql-client-common libllvm17t64
for f in *.deb; do dpkg-deb -x "$f" ~/.local/opt/postgresql-16; done

# OCaml's archive, from o1Labs' apt repository (noble, component stable);
# check the SHA256 against dists/noble/stable/binary-amd64/Packages.
curl -O https://packages.o1test.net/pool/noble/m/mi/mina-archive-generic_4.0.2-84b8d96_amd64.deb
echo "81eac784936acbe022cc6a6cb0116ab94e800d434a0936ce2ca3ff32c1bf1182  mina-archive-generic_4.0.2-84b8d96_amd64.deb" | sha256sum -c
dpkg-deb -x mina-archive-generic_4.0.2-84b8d96_amd64.deb ~/.local/opt/mina-archive-4.0.2
```

It also needs `libpq.so.5` and `libjemalloc`, which `~/.local/opt/mina-4.0.2`
already has.

## Run

`env.sh` holds the ports (Postgres 3445, archive 3446 by default: pick ones in
your slot) and `E2E`, the run directory (default `.work/e2e`).

```bash
A=tools/regression/archive
$A/postgres.sh start                    # first run: initdb and the schema
$A/archive.sh & echo $! > .work/e2e/archive.pid
MINA=target/release/mina \
MINA_GENESIS_CONFIG=$WORK/genesis-mainnet/mesa-mainnet-without-ledgers.json \
  $A/node.sh 3440 8640 & echo $! > .work/e2e/node.pid
curl -s localhost:3440/archive/status    # the outbox
$A/psql.sh -c "select count(*), min(height), max(height) from blocks"
```

Stop the archive by its PID to simulate downtime, start it again with
`archive.sh`, and stop or `kill -9` the node by its PID to test a restart; the
outbox is in `.work/e2e/home/archive-outbox`. Stop everything by PID and with
`postgres.sh stop` when done.

`mina-missing-blocks-auditor` refuses a database without the fork's first block;
until one is imported, check missing parents with
`select height, state_hash from blocks where parent_id is null`.
