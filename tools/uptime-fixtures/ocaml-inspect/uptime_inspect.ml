(* Decode public uptime captures using the daemon's OCaml types and
   verify the outer signature. Run delegation_verify for full proof checks. *)
open Core
open Mina_base

let hex s = `String (String.concat (List.map (String.to_list s) ~f:(fun c -> Printf.sprintf "%02x" (Char.to_int c))))

let digest_hex digest =
  match Sok_message.Digest.to_yojson digest with
  | `String s -> hex s
  | _ -> failwith "Unexpected SOK digest JSON encoding"

(* Invert payload.ml:25-36 with the public submitter, never a private key. *)
let verify_http_signature ~signature_kind request =
  let open Yojson.Safe.Util in
  let data = member "data" request |> Yojson.Safe.to_string in
  let digest = Blake2.digest_string data in
  let input : (Snark_params.Tick.Field.t, bool) Random_oracle.Legacy.Input.t =
    { field_elements = [||]
    ; bitstrings = [| Blake2.to_raw_string digest |> Blake2.string_to_bits |> Array.to_list |]
    }
  in
  let signature = member "signature" request |> to_string |> Signature.of_base58_check_exn in
  let public_key = member "submitter" request |> to_string
    |> Signature_lib.Public_key.Compressed.of_base58_check_exn
    |> Signature_lib.Public_key.decompress_exn
  in
  Signature_lib.Schnorr.Legacy.verify ~signature_kind
    signature (Snark_params.Tick.Inner_curve.of_affine public_key) input

let inspect ?(signature_kind=Mina_signature_kind.Testnet) path =
  let request = Yojson.Safe.from_file path in
  let open Yojson.Safe.Util in
  let data = member "data" request in
  let raw_block = data |> member "block" |> to_string |> Base64.decode_exn in
  let block = Binable.of_string (module Mina_block.Stable.Latest) raw_block in
  let header = Mina_block.Stable.Latest.header block in
  let state = Mina_block.Header.protocol_state header in
  let consensus = Mina_state.Protocol_state.consensus_state state in
  let blockchain = Mina_state.Protocol_state.blockchain_state state in
  let ledger_hash =
    Mina_state.Blockchain_state.staged_ledger_hash blockchain
    |> Staged_ledger_hash.ledger_hash
  in
  let state_hash =
    Mina_state.Protocol_state.hashes state
    |> State_hash.State_hashes.state_hash
    |> State_hash.to_base58_check
  in
  let submitter = member "submitter" request |> to_string in
  let snark_work =
    match member "snark_work" data with
    | `Null -> `Null
    | `String encoded ->
        let raw = Base64.decode_exn encoded in
        let { Uptime_service.Proof_data.proof; proof_time; snark_work_fee } =
          Binable.of_string (module Uptime_service.Proof_data) raw
        in
        let message =
          Sok_message.create ~fee:snark_work_fee
            ~prover:(Signature_lib.Public_key.Compressed.of_base58_check_exn submitter)
        in
        let proof_digest = Ledger_proof.sok_digest proof in
        let expected_digest = Sok_message.digest message in
        let statement = Ledger_proof.statement proof in
        `Assoc
          [ ("decoded_bytes", `Int (String.length raw))
          ; ("ledger_proof_bytes", `Int (Ledger_proof.Stable.Latest.bin_size_t proof))
          ; ("proof_time_seconds", `Float (Time.Span.to_sec proof_time))
          ; ("fee", Currency.Fee.to_yojson snark_work_fee)
          ; ("sok_message", Sok_message.to_yojson message)
          ; ("sok_message_bin_prot_hex", hex (Binable.to_string (module Sok_message.Stable.Latest) message))
          ; ("proof_sok_digest_hex", digest_hex proof_digest)
          ; ("expected_sok_digest_hex", digest_hex expected_digest)
          ; ("sok_matches", `Bool (Sok_message.Digest.equal proof_digest expected_digest))
          ; ("sok_is_default", `Bool (Sok_message.Digest.equal proof_digest Sok_message.Digest.default))
          ; ("statement", Mina_state.Snarked_ledger_state.to_yojson statement)
          ; ("target_matches_block", `Bool (Pasta_bindings.Fp.equal statement.target.second_pass_ledger ledger_hash))
          ; ("roundtrip_exact", `Bool (String.equal raw (Binable.to_string (module Uptime_service.Proof_data) {proof; proof_time; snark_work_fee})))
          ]
    | _ -> failwith "snark_work must be absent or a string"
  in
  `Assoc
    [ ("http_signature_valid", `Bool (verify_http_signature ~signature_kind request))
    ; ("state_hash", `String state_hash)
    ; ("height", `Int (Unsigned.UInt32.to_int (Consensus.Data.Consensus_state.blockchain_length consensus)))
    ; ("global_slot_since_genesis", `Int (Mina_numbers.Global_slot_since_genesis.to_int (Consensus.Data.Consensus_state.global_slot_since_genesis consensus)))
    ; ("staged_ledger_ledger_hash", Ledger_hash.to_yojson ledger_hash)
    ; ("block_decoded_bytes", `Int (String.length raw_block))
    ; ("block_roundtrip_exact", `Bool (String.equal raw_block (Binable.to_string (module Mina_block.Stable.Latest) block)))
    ; ("created_at", member "created_at" data)
    ; ("submitter", `String submitter)
    ; ("snark_work", snark_work)
    ]

let () =
  match Array.to_list (Sys.get_argv ()) with
  | [_; "job"; input; output] ->
      let module S = Snark_work_lib.Spec.Single.Stable.Latest in
      let job = Yojson.Safe.from_file input |> S.of_yojson |> Result.ok_or_failwith in
      Out_channel.write_all output ~data:(Binable.to_string (module S) job)
  | [_; path] -> inspect path |> Yojson.Safe.pretty_to_channel Out_channel.stdout
  | [_; path; "--signature-kind"; kind] ->
      let signature_kind = match kind with
        | "mainnet" -> Mina_signature_kind.Mainnet
        | "testnet" -> Mina_signature_kind.Testnet
        | _ -> failwith "signature kind must be mainnet or testnet"
      in
      inspect ~signature_kind path |> Yojson.Safe.pretty_to_channel Out_channel.stdout
  | _ -> failwith "usage: uptime_inspect PAYLOAD.json [--signature-kind mainnet|testnet]"
