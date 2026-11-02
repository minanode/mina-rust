  (* Test adapter inserted inside Make_str, before interface sealing.
     All consensus calculations below call the unmodified 4.0.4 functions. *)
  let consensus_diff json =
    let open Yojson.Safe.Util in
    let open Data in
    let get k = member k json in
    let int k = get k |> to_int in
    let str k = get k |> to_string in
    let ok = function Ok x -> x | Error s -> failwith s in
    let module Profile = (val Genesis_constants.profiled ()) in
    let constraint_constants = Profile.constraint_constants in
    let constants = Constants.create ~constraint_constants
      ~protocol_constants:Profile.genesis_constants.protocol in
    let gs n = Global_slot.of_slot_number ~constants
      (Mina_numbers.Global_slot_since_hard_fork.of_int n) in
    let state k = Consensus_state.Value.of_yojson (get k) |> ok in
    let field k = Tick.Field.of_string (str k) in
    let balance k = Balance.of_uint64 (UInt64.of_string (str k)) in
    let amount k = Amount.of_uint64 (UInt64.of_string (str k)) in
    let key n = Public_key.of_private_key_exn (Consensus_vrf.Scalar.of_string (Int.to_string n))
      |> Public_key.compress in
    let epoch_json d = Epoch_data.Staking_value_versioned.Value.to_yojson d in
    match str "op" with
    | "account_defaults" -> `Assoc [
        "empty", Mina_base.Account.to_yojson Mina_base.Account.empty;
        "empty_hash", `String (Tick.Field.to_string (Mina_base.Account.digest Mina_base.Account.empty))]
    | "constants" -> `Assoc [
        "slots_per_epoch", `Int (Length.to_int constants.slots_per_epoch);
        "slots_per_sub_window", `Int (Length.to_int constants.slots_per_sub_window);
        "sub_windows_per_window", `Int (Length.to_int constants.sub_windows_per_window);
        "grace_period_end", `Int (Mina_numbers.Global_slot_since_hard_fork.to_int constants.grace_period_end);
        "slot_duration_ms", `Int (Int64.to_int (Block_time.Span.to_ms constants.slot_duration_ms))]
    | "arithmetic" ->
        let slot = gs (int "slot") in
        let epoch, within = Global_slot.to_epoch_and_slot slot in
        `Assoc ["epoch", `Int (Epoch.to_int epoch);
                "slot", `Int (Slot.to_int within);
                "global_sub_window", `Int (UInt32.to_int (Global_sub_window.sub (Global_sub_window.of_global_slot ~constants slot) (Global_sub_window.constant UInt32.zero)));
                "relative_sub_window", `Int (Sub_window.to_int (Global_sub_window.sub_window ~constants (Global_sub_window.of_global_slot ~constants slot)));
                "checkpoint_window", `Int (UInt32.to_int (Consensus_state.checkpoint_window ~constants slot));
                "seed_update", `Bool (Slot.in_seed_update_range ~constants within)]
    | "epoch_bounds" ->
        let within = Global_slot.slot (gs (int "slot")) |> Slot.to_int in
        `String (if within = 0 then "Beginning" else if within = Length.to_int constants.slots_per_epoch - 1 then "End" else "Within")
    | "density" ->
        let minimum, buckets = Min_window_density.update_min_window_density
          ~incr_window:(get "incr" |> to_bool) ~constants
          ~prev_global_slot:(gs (int "prev")) ~next_global_slot:(gs (int "next"))
          ~prev_sub_window_densities:(get "buckets" |> to_list |> List.map ~f:(fun j -> Length.of_int (to_int j)))
          ~prev_min_window_density:(Length.of_int (int "minimum")) in
        `Assoc ["minimum", `Int (Length.to_int minimum);
                "buckets", `List (List.map buckets ~f:(fun n -> `Int (Length.to_int n)))]
    | "select" ->
        let existing = state "a" and candidate = state "b" in
        let hashed state k = { With_hash.data = state; hash =
          { Mina_base.State_hash.State_hashes.state_hash = Mina_base.State_hash.of_yojson (get k) |> ok;
            state_body_hash = None } } in
        let module Context = struct
          let logger = Logger.null ()
          let constraint_constants = constraint_constants
          let consensus_constants = constants
        end in
        let choice = Hooks.select ~context:(module Context)
          ~existing:(hashed existing "a_hash") ~candidate:(hashed candidate "b_hash") in
        `Assoc ["take", `Bool (match choice with `Take -> true | `Keep -> false);
                "short", `Bool (Hooks.is_short_range ~constants existing candidate)]
    | "epoch_select" ->
        (match Hooks.select_epoch_data ~consensus_state:(state "state") ~epoch:(Epoch.of_int (int "epoch")) with
        | Ok d -> epoch_json d | Error () -> `Null)
    | "epoch_update" ->
        let s = state "state" in
        let staking, next, count = Epoch_data.update_pair
          (s.staking_epoch_data, s.next_epoch_data) s.epoch_count
          ~prev_epoch:(Consensus_state.curr_epoch s)
          ~next_epoch:(Global_slot.epoch (gs (int "next")))
          ~next_slot:(Global_slot.slot (gs (int "next")))
          ~prev_protocol_state_hash:(Mina_base.State_hash.of_yojson (get "parent_hash") |> ok)
          ~producer_vrf_result:(field "vrf_hash")
          ~snarked_ledger_hash:(Mina_base.Frozen_ledger_hash.of_yojson (get "snarked") |> ok)
          ~genesis_ledger_hash:(Mina_base.Frozen_ledger_hash.of_yojson (get "genesis") |> ok)
          ~total_currency:(amount "total") ~constants in
        `Assoc ["staking", epoch_json staking; "next", epoch_json next;
                "count", `Int (Length.to_int count)]
    | "vrf" ->
        let message = Consensus_vrf.Message.{global_slot = Mina_numbers.Global_slot_since_hard_fork.of_int (int "slot");
          seed = Mina_base.Epoch_seed.of_hash (field "seed"); delegator = int "index"} in
        (* Public synthetic scalar identifiers only; never load a key file. *)
        let output = Consensus_vrf.Integrated.eval ~constraint_constants
          ~private_key:(Consensus_vrf.Scalar.of_string (Int.to_string (int "key_id" + 1))) message in
        let truncated = Consensus_vrf.Output.truncate output in
        `Assoc ["hash", `String (Tick.Field.to_string output);
                "truncated", `String (String.to_list truncated |> List.map ~f:(fun c -> Printf.sprintf "%02x" (Char.to_int c)) |> String.concat);
                "won", `Bool (Consensus_vrf.Threshold.is_satisfied
                  ~my_stake:(balance "stake") ~total_stake:(amount "total") truncated)]
    | "threshold" ->
        let hex = str "output" in
        let raw = String.init (String.length hex / 2) ~f:(fun i ->
          Char.of_int_exn (Int.of_string ("0x" ^ String.sub hex ~pos:(i * 2) ~len:2))) in
        `Bool (Consensus_vrf.Threshold.is_satisfied
          ~my_stake:(balance "stake") ~total_stake:(amount "total") raw)
    | "delegation" ->
        let ledger = Mina_ledger.Ledger.create ~depth:constraint_constants.ledger_depth () in
        Exn.protect ~finally:(fun () -> Mina_ledger.Ledger.close ledger) ~f:(fun () ->
          List.iter (get "accounts" |> to_list) ~f:(fun j ->
            let i k = member k j |> to_int in
            let pk = key (i "owner" + 1) in
            let token_id = Mina_base.Token_id.of_field (Tick.Field.of_int (i "token")) in
            let id = Mina_base.Account_id.create pk token_id in
            let account = Mina_base.Account.create id
              (Balance.of_uint64 (UInt64.of_string (member "balance" j |> to_string))) in
            let delegate = match member "delegate" j with `Null -> None | d -> Some (key (to_int d + 1)) in
            let account = { account with delegate } in
            ignore (Mina_ledger.Ledger.get_or_create_account ledger id account |> Or_error.ok_exn : _));
          let producer = key (int "producer" + 1) in
          let table = compute_delegatee_table_genesis_ledger (Public_key.Compressed.Set.singleton producer) ledger in
          let entries = Hashtbl.find table producer |> Option.value_map ~default:[] ~f:Hashtbl.to_alist in
          `List (List.sort entries ~compare:(fun (a,_) (b,_) -> Mina_base.Account.Index.compare a b)
            |> List.map ~f:(fun (i,(a : Mina_base.Account.t)) -> `Assoc ["index", `Int (Mina_base.Account.Index.to_int i);
              "public_key", Public_key.Compressed.to_yojson a.public_key;
              "balance", `String (UInt64.to_string (Balance.to_uint64 a.balance))])))
    | op -> failwith ("unknown operation: " ^ op)
