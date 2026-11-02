// Build proof-authorised zkApp commands whose proofs use one custom gate
// each (xor, rot, foreign field multiplication, the lookup gate) on an o1js
// LocalBlockchain with real proofs and testnet signatures, as devnet's, and
// write the deploy command, every method command and the fee payer's
// starting balance to OUT_DIR/zkapps.json, the input of
// crates/cli/examples/zkapp_gate_jobs.rs. Written for o1js 3.1.0.
//
//   node gen.mjs O1JS_DIST_NODE_INDEX_JS OUT_DIR
//
// It writes no key. The fee payer is a LocalBlockchain test account and the
// zkApp key is random; neither holds value on any network.
import { pathToFileURL } from "node:url";
import fs from "node:fs";
import path from "node:path";

const [o1jsIndex, outDir] = process.argv.slice(2);
const o1js = await import(pathToFileURL(o1jsIndex).href);
const {
  SmartContract,
  declareMethods,
  Field,
  Gadgets,
  Mina,
  PrivateKey,
  AccountUpdate,
  Provable,
} = o1js;

// secp256k1's base field, a 256-bit foreign modulus.
const f = (1n << 256n) - (1n << 32n) - 0b1111010001n;

class Gates extends SmartContract {
  async xorGate(a, b) {
    Gadgets.xor(a, b, 64);
  }
  async rotGate(a) {
    Gadgets.rotate64(a, 3, "left");
  }
  async ffmulGate() {
    const x = Provable.witness(Gadgets.Field3, () => f - 1n);
    const y = Provable.witness(Gadgets.Field3, () => f - 2n);
    Gadgets.ForeignField.assertAlmostReduced([x, y], f);
    Gadgets.ForeignField.mul(x, y, f);
  }
  // Raw lookup gates need a constraint system: o1js also runs the method
  // body outside one to build the account update, so emit them only when
  // compiling or proving. The 12-bit table is in a circuit only with a range
  // check gate, hence `rangeCheck64`.
  async lookupGate(a) {
    Gadgets.rangeCheck64(a);
    if (Provable.inCheckedComputation())
      Gadgets.rangeCheck3x12(a, Field(0), Field(0));
  }
}
declareMethods(Gates, {
  xorGate: [Field, Field],
  rotGate: [Field],
  ffmulGate: [],
  lookupGate: [Field],
});

const t0 = Date.now();
const log = (...a) =>
  console.log(`[${((Date.now() - t0) / 1000).toFixed(1)}s]`, ...a);

const Local = await Mina.LocalBlockchain({ proofsEnabled: true });
Mina.setActiveInstance(Local);
const sender = Local.testAccounts[0];
const genesisAccount = Mina.getAccount(sender);
log("network id", Mina.getNetworkId(), "fee payer", sender.toBase58());

const analysis = await Gates.analyzeMethods();
for (const [name, a] of Object.entries(analysis)) {
  log(
    "method",
    name,
    "rows",
    a.rows,
    "gates",
    [...new Set(a.gates.map(g => g.type))].join(","),
  );
}
const { verificationKey } = await Gates.compile();
log("compiled; vk hash", verificationKey.hash.toString());

const zkappKey = PrivateKey.random();
const zkappAddress = zkappKey.toPublicKey();
const zkapp = new Gates(zkappAddress);

// The input of crates/cli/examples/zkapp_gate_jobs.rs.
const out = {
  network_id: Mina.getNetworkId(),
  zkapp: zkappAddress.toBase58(),
  vk_hash: verificationKey.hash.toString(),
  fund: [
    {
      public_key: sender.toBase58(),
      balance: genesisAccount.balance.toString(),
    },
  ],
  commands: [],
};

const deploy = await Mina.transaction(
  { sender, fee: 100_000_000 },
  async () => {
    AccountUpdate.fundNewAccount(sender);
    await zkapp.deploy();
  },
);
await deploy.prove();
deploy.sign([sender.key, zkappKey]);
await (await deploy.send()).wait();
out.commands.push({ name: "deploy", graphql: JSON.parse(deploy.toJSON()) });
log("deployed");

const calls = [
  ["xor", () => zkapp.xorGate(Field(0xdeadbeefn), Field(0x12345678n))],
  ["rot", () => zkapp.rotGate(Field(0xfedcba9876543210n))],
  ["ffmul", () => zkapp.ffmulGate()],
  ["lookup", () => zkapp.lookupGate(Field(1234))],
];
for (const [name, call] of calls) {
  const tx = await Mina.transaction({ sender, fee: 100_000_000 }, call);
  await tx.prove();
  tx.sign([sender.key]);
  await (await tx.send()).wait();
  out.commands.push({ name, graphql: JSON.parse(tx.toJSON()) });
  log("proved and applied", name);
}

fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(
  path.join(outDir, "zkapps.json"),
  JSON.stringify(out, null, 2) + "\n",
);
log("wrote", path.join(outDir, "zkapps.json"));
