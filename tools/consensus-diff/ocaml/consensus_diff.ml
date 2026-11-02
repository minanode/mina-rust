open Core_kernel

let () =
  In_channel.iter_lines In_channel.stdin ~f:(fun line ->
    let response = try Consensus.consensus_diff (Yojson.Safe.from_string line)
      with exn -> `Assoc ["error", `String (Exn.to_string exn)] in
    Out_channel.output_string Out_channel.stdout (Yojson.Safe.to_string response ^ "\n");
    Out_channel.flush Out_channel.stdout)
