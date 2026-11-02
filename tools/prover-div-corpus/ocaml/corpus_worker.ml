(* Offline adapter; the production worker and verifier are unmodified.
   OCaml: https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/snark_worker/prod.ml *)
open Core
open Async
open Mina_base

let write_json path value =
  Out_channel.write_all path ~data:(Yojson.Safe.pretty_to_string value)

let worker_cache = ref None

let run network command input output message_path proof_path =
  let signature_kind = match network with
    | "mainnet" -> Mina_signature_kind.Mainnet
    | "devnet" -> Mina_signature_kind.Testnet
    | _ -> failwith "unknown network"
  in
  let module Constants = (val Genesis_constants.profiled ()) in
  let start = Time.now () in
  let%bind worker = match !worker_cache with
    | Some worker -> Deferred.return worker
    | None ->
        let%map worker = Snark_worker.Impl.Worker_state.create
          ~constraint_constants:Constants.constraint_constants
          ~proof_level:Genesis_constants.Proof_level.Full ~signature_kind () in
        worker_cache := Some worker;
        worker
  in
  let load_seconds = Time.Span.to_sec (Time.diff (Time.now ()) start) in
  let module S = Snark_work_lib.Spec.Single.Stable.Latest in
  let raw = In_channel.read_all input in
  let spec = Binable.of_string (module S) raw in
  assert (String.equal raw (Binable.to_string (module S) spec));
  let message = Yojson.Safe.from_file message_path |> Sok_message.of_yojson |> Result.ok_or_failwith in
  let statement = match spec with
    | Transition (s, _) -> s
    | Merge (s, _, _) -> s
  in
  match command with
  | "prove" ->
      let%map proof, elapsed = Snark_worker.Impl.perform_single worker ~message spec >>| Or_error.ok_exn in
      Out_channel.write_all output ~data:(Binable.to_string (module Ledger_proof.Stable.Latest) proof);
      write_json (output ^ ".json") (`Assoc ["load_seconds", `Float load_seconds; "proof_seconds", `Float (Time.Span.to_sec elapsed)]);
      0
  | "verify" ->
      let (module T) = match worker.proof_level_snark with
        | Full m -> m
        | _ -> failwith "full proof level required"
      in
      let proof = In_channel.read_all proof_path |> Binable.of_string (module Ledger_proof.Stable.Latest) in
      let statement_matches = Transaction_snark.Statement.equal (Ledger_proof.statement proof) statement in
      let sok_matches = Sok_message.Digest.equal (Ledger_proof.sok_digest proof) (Sok_message.digest message) in
      let%map result = T.verify [proof] in
      let accepted = statement_matches && sok_matches && Result.is_ok result in
      write_json output (`Assoc ["accepted", `Bool accepted; "statement_matches", `Bool statement_matches;
        "sok_matches", `Bool sok_matches; "verification_error", (match result with Ok () -> `Null | Error e -> `String (Error.to_string_hum e))]);
      if accepted then 0 else 1
  | _ -> failwith "unknown command"

let rec serve network =
  match%bind Reader.read_line (Lazy.force Reader.stdin) with
  | `Eof -> Shutdown.exit 0
  | `Ok line ->
      let%bind result = Monitor.try_with (fun () ->
        match Yojson.Safe.from_string line |> Yojson.Safe.Util.to_list
          |> List.map ~f:Yojson.Safe.Util.to_string with
        | [requested_network; command; input; output; message; proof] ->
            if not (String.equal network requested_network) then failwith "server network mismatch";
            run network command input output message proof
        | _ -> failwith "invalid request") in
      let response = match result with
        | Ok code -> `Assoc ["exit_code", `Int code]
        | Error error -> `Assoc ["exit_code", `Int 2; "error", `String (Exn.to_string error)]
      in
      printf "CORPUS_RESULT %s\n%!" (Yojson.Safe.to_string response);
      serve network

let () =
  match Array.to_list (Sys.get_argv ()) with
  | [_; network; "serve"] ->
      don't_wait_for (serve network);
      never_returns (Scheduler.go ())
  | [_; network; command; input; output; message; proof] ->
      don't_wait_for (run network command input output message proof >>= Shutdown.exit);
      never_returns (Scheduler.go ())
  | _ -> failwith "usage: corpus_worker NETWORK prove|verify JOB OUTPUT MESSAGE PROOF"
