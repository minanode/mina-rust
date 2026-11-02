(* Exercise OCaml 4.0.3's uptime zkApp path on a public base-job witness.
   No daemon, keys, network submission or altered protocol implementation. *)
open Core
open Async
open Mina_base

module Base_job = struct
  type t =
    Mina_state.Snarked_ledger_state.Stable.Latest.t
    * Transaction_witness.Stable.Latest.t
  [@@deriving bin_io_unversioned]
end

let run input output =
  let module Constants = (val Genesis_constants.profiled ()) in
  let module S = Transaction_snark.Make (struct
    let signature_kind = Mina_signature_kind.Testnet
    let constraint_constants = Constants.constraint_constants
    let proof_level = Genesis_constants.Proof_level.Full
  end) in
  let statement, witness =
    In_channel.read_all input |> Binable.of_string (module Base_job)
  in
  let zkapp_command =
    match witness.transaction with
    | Command (Zkapp_command command) -> command
    | _ -> failwith "expected a zkApp base job"
  in
  let cache = Proof_cache_tag.create_identity_db () in
  let zkapp_command =
    Zkapp_command.write_all_proofs_to_disk ~signature_kind:Testnet
      ~proof_cache_db:cache zkapp_command
  in
  let staged_ledger_hash =
    Staged_ledger_hash.of_aux_ledger_and_coinbase_hash
      Staged_ledger_hash.Aux_hash.dummy statement.target.second_pass_ledger
      (Pending_coinbase.create ~depth:Constants.constraint_constants.pending_coinbase_depth () |> Or_error.ok_exn)
  in
  let witness, spec, statement =
    Uptime_service.Uptime_snark_worker.extract_terminal_zk_segment
      ~m:(module S) ~witness ~input:statement ~zkapp_command ~staged_ledger_hash
    |> Or_error.ok_exn
  in
  (* Identical to uptime_snark_worker.ml:95-109: no with_sok_digest. *)
  let%bind proof, proof_time =
    Snark_worker.Impl.measure_runtime ~logger:(Logger.create ())
      ~spec_json:(lazy ("spec", Transaction_snark.Zkapp_command_segment.Basic.Stable.Latest.to_yojson spec))
      (fun () ->
        let witness =
          Transaction_witness.Zkapp_command_segment_witness.write_all_proofs_to_disk
            witness ~proof_cache_db:cache ~signature_kind:Testnet
        in
        S.of_zkapp_command_segment_exn ~statement ~witness ~spec
        |> Deferred.map ~f:Result.return)
    >>| Or_error.ok_exn
  in
  let%map verified = S.verify [proof] in
  Or_error.ok_exn verified;
  let data : Uptime_service.Proof_data.t =
    { proof; proof_time; snark_work_fee = Currency.Fee.of_mina_string_exn "0.001" }
  in
  Out_channel.write_all output ~data:(Binable.to_string (module Uptime_service.Proof_data) data);
  let zero = Sok_message.Digest.equal (Ledger_proof.sok_digest proof) Sok_message.Digest.default in
  printf "cryptographic_verification=ok sok_is_default=%b proof_seconds=%.17g\n"
    zero (Time.Span.to_sec proof_time)

let () =
  match Array.to_list (Sys.get_argv ()) with
  | [_; "verify"; input] ->
      let module Constants = (val Genesis_constants.profiled ()) in
      let module S = Transaction_snark.Make (struct
        let signature_kind = Mina_signature_kind.Testnet
        let constraint_constants = Constants.constraint_constants
        let proof_level = Genesis_constants.Proof_level.Full
      end) in
      let data = In_channel.read_all input
        |> Binable.of_string (module Uptime_service.Proof_data) in
      don't_wait_for (
        S.verify [data.proof] >>= fun result ->
        Or_error.ok_exn result;
        printf "cryptographic_verification=ok sok_is_default=%b\n"
          (Sok_message.Digest.equal (Ledger_proof.sok_digest data.proof) Sok_message.Digest.default);
        Shutdown.exit 0);
      never_returns (Scheduler.go ())
  | [_; input; output] ->
      don't_wait_for (run input output >>= fun () -> Shutdown.exit 0);
      never_returns (Scheduler.go ())
  | _ -> failwith "usage: uptime_prove BASE_JOB.bin OUTPUT.bin | verify PROOF_DATA.bin"
