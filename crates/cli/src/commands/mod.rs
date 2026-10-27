pub mod build_info;
pub mod fork;
pub mod internal;
pub mod keys;
pub mod ledger;
pub mod node;
pub mod node_api;
pub mod replay;
pub mod report;
mod secret_value;
pub mod wallet;

/// The crate version and the WebRTC backend, for `mina --version`.
fn version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        format!(
            "{} (WebRTC: {})",
            env!("CARGO_PKG_VERSION"),
            mina_node::p2p::service_impl::webrtc::BACKEND
        )
    })
}

#[derive(Debug, clap::Parser)]
#[command(
    name = "mina",
    about = "Mina node in Rust, with its wallet, ledger and operator tools",
    version = version()
)]
pub struct MinaCli {
    #[arg(
        global = true,
        long,
        value_enum,
        default_value_t = Network::Devnet,
        env = "MINA_NETWORK",
        help_heading = "Global options"
    )]
    /// The network whose chain and keys the command uses.
    pub network: Network,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum Network {
    Devnet,
    Mainnet,
}

#[derive(Debug, clap::Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    // The about text is the struct's, which has more than one paragraph.
    Node(node::Node),
    /// Generate or print account and libp2p key pairs.
    #[command(alias = "misc")]
    Keys(keys::Keys),
    /// Replay a node recorded with `node --record`, for development.
    #[command(hide = true)]
    Replay(replay::Replay),
    /// Print this binary's version, commit and build details.
    BuildInfo(build_info::Command),
    /// Create keys, check balances and send transactions through a node.
    Wallet(wallet::Wallet),
    /// Export a node's ledgers, and hash or total an exported ledger, as
    /// OCaml's `mina ledger`.
    Ledger(ledger::Ledger),
    /// Write the next hard fork's config from a running node, as OCaml's
    /// `mina advanced generate-hardfork-config`.
    Fork(fork::Fork),
    /// Development tools: GraphQL introspection and the CLI reference.
    #[command(hide = true)]
    Internal(internal::Internal),
    Report(report::ReportCommand),
    Triage(report::TriageCommand),
}

impl Command {
    pub fn run(self, network: Network) -> anyhow::Result<()> {
        match self {
            Self::Node(v) => v.run(),
            Self::Keys(v) => v.run(),
            Self::Replay(v) => v.run(),
            Self::BuildInfo(v) => v.run(),
            Self::Wallet(v) => v.run(network),
            Self::Ledger(v) => v.run(network),
            Self::Fork(v) => v.run(network),
            Self::Internal(v) => v.run(),
            Self::Report(v) => v.run(),
            Self::Triage(v) => v.run(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{node, node_api, wallet, Command, MinaCli};

    fn parse(args: &[&str]) -> Result<MinaCli, clap::Error> {
        <MinaCli as clap::Parser>::try_parse_from(
            std::iter::once("mina").chain(args.iter().copied()),
        )
    }

    // Key passwords come from the environment only. A password given as an
    // argument is refused while parsing, before any command runs, without
    // being echoed, and the message names the variable to use instead.
    #[test]
    fn password_arguments_are_refused() {
        const VALUE: &str = "dummy-refused-password";
        let privkey: &[&[&str]] = &[
            &["keys", "mina-encrypted-key", "--output", "k"],
            &["misc", "mina-encrypted-key", "--file", "k"],
            &["wallet", "address", "--from", "k"],
            &["wallet", "balance", "--from", "k"],
            &["wallet", "generate", "--output", "k"],
            &["wallet", "send", "--from", "k"],
        ];
        let node: &[&[&str]] = &[&["node"], &["node", "--producer-key", "k"]];
        for (commands, names) in [
            (privkey, &["MINA_PRIVKEY_PASS"][..]),
            (node, &["MINA_PRIVKEY_PASS", "MINA_LIBP2P_PASS"][..]),
        ] {
            let joined = format!("--privkey-password={VALUE}");
            let flags = [
                vec![VALUE],
                vec!["--password", VALUE],
                vec!["--password"],
                vec!["--privkey-pass", VALUE],
                vec![joined.as_str()],
                vec!["--pass", VALUE],
            ];
            let cases = commands
                .iter()
                .flat_map(|args| flags.iter().map(move |flag| (*args, flag)));
            for (args, flag) in cases {
                let error = parse(&[args, flag.as_slice()].concat()).unwrap_err();
                assert_eq!(
                    error.kind(),
                    clap::error::ErrorKind::ValueValidation,
                    "{args:?}"
                );
                let message = error.to_string();
                assert!(!message.contains(VALUE), "{args:?}");
                assert!(message.contains("not the command line"), "{args:?}");
                assert!(message.ends_with('\n'), "{args:?}");
                for name in names {
                    assert!(message.contains(name), "{args:?}: {name}");
                }
            }
        }
    }

    fn find<'a>(root: &'a clap::Command, path: &[&str]) -> &'a clap::Command {
        path.iter().fold(root, |command, name| {
            command.find_subcommand(name).expect("subcommand")
        })
    }

    fn walk<'a>(command: &'a clap::Command, out: &mut Vec<&'a clap::Command>) {
        out.push(command);
        for sub in command.get_subcommands() {
            walk(sub, out);
        }
    }

    // CLI-REF: every command that talks to a running node takes `--node`,
    // the node's HTTP base URL, and reads MINA_NODE_URL.
    #[test]
    fn node_url_commands_read_mina_node_url() {
        let root = <MinaCli as clap::CommandFactory>::command();
        for path in [
            &["node", "status"][..],
            &["wallet", "balance"],
            &["wallet", "delegate"],
            &["wallet", "send"],
            &["wallet", "status"],
            &["ledger", "export"],
            &["fork", "export"],
            &["report"],
            &["triage"],
            &["internal", "graphql", "list"],
            &["internal", "graphql", "inspect"],
            &["internal", "graphql", "run"],
        ] {
            let node = find(&root, path)
                .get_arguments()
                .find(|arg| arg.get_long() == Some("node"))
                .expect("--node");
            assert_eq!(
                node.get_env(),
                Some(std::ffi::OsStr::new("MINA_NODE_URL")),
                "{path:?}"
            );
            assert_eq!(
                node.get_value_names().map(|names| names[0].as_str()),
                Some("URL"),
                "{path:?}"
            );
        }
        let args = [
            "wallet",
            "status",
            "--hash",
            "h",
            "--node",
            "http://127.0.0.1:1",
        ];
        assert!(parse(&args).is_ok());
    }

    // CLI-REF: `--url` and `--endpoint` stay accepted, hidden.
    #[test]
    fn older_node_url_names_are_hidden_aliases() {
        let url = "http://127.0.0.1:1";
        let Command::Node(node) = parse(&["node", "status", "--url", url]).unwrap().command else {
            panic!("node");
        };
        let Some(node::NodeCommand::Status(status)) = node.command else {
            panic!("status");
        };
        assert_eq!(status.node.as_str(), "http://127.0.0.1:1/");
        let Command::Wallet(wallet) =
            parse(&["wallet", "balance", "--address", "B62", "--endpoint", url])
                .unwrap()
                .command
        else {
            panic!("wallet");
        };
        let wallet::WalletCommand::Balance(balance) = wallet.command else {
            panic!("balance");
        };
        assert_eq!(balance.node, url);

        let root = <MinaCli as clap::CommandFactory>::command();
        for (path, alias) in [
            (&["node", "status"][..], "url"),
            (&["wallet", "balance"], "endpoint"),
        ] {
            let node = find(&root, path)
                .get_arguments()
                .find(|arg| arg.get_long() == Some("node"))
                .unwrap();
            assert!(node.get_visible_aliases().unwrap_or_default().is_empty());
            assert_eq!(node.get_all_aliases(), Some(vec![alias]), "{path:?}");
        }
    }

    // GraphQL goes to `<node>/graphql`; a value that already names the
    // GraphQL path, as `--endpoint` took, is kept as given.
    #[test]
    fn graphql_urls_are_derived_from_the_node_url() {
        for (node, url) in [
            ("http://localhost:3000", "http://localhost:3000/graphql"),
            ("http://localhost:3000/", "http://localhost:3000/graphql"),
            (
                "http://localhost:3000/graphql",
                "http://localhost:3000/graphql",
            ),
            ("https://n.example/mina/", "https://n.example/mina/graphql"),
        ] {
            assert_eq!(node_api::graphql_url(node), url, "{node}");
        }
    }

    // CLI-REF: every environment variable the CLI reads has the MINA_
    // prefix, so a bare `PORT` or `CONFIG` set for other software never
    // reaches the node. Nothing warns about the bare names.
    #[test]
    fn every_environment_variable_has_the_mina_prefix() {
        let root = <MinaCli as clap::CommandFactory>::command();
        let mut commands = Vec::new();
        walk(&root, &mut commands);
        let mut names = Vec::new();
        for command in commands {
            for arg in command.get_arguments() {
                if let Some(env) = arg.get_env() {
                    let env = env.to_str().unwrap();
                    assert!(env.starts_with("MINA_"), "{} {env}", command.get_name());
                    names.push(env);
                }
            }
        }
        for name in [
            "MINA_PORT",
            "MINA_LIBP2P_PORT",
            "MINA_SEED",
            "MINA_CONFIG",
            "MINA_GENESIS_CONFIG",
            "MINA_PEER_LIST_FILE",
            "MINA_PEER_LIST_URL",
            "MINA_PRODUCER_KEY",
            "MINA_RUN_SNARKER",
            "MINA_SNARKER_FEE",
            "MINA_SNARKER_STRATEGY",
            "MINA_SNARKER_BLOCK_FIRST_LEAD",
            "MINA_VERBOSITY",
            "MINA_RECORD",
            "MINA_RNG_SEED",
            "MINA_ARCHIVE_LOCAL_STORAGE",
            "MINA_ARCHIVE_ARCHIVER_PROCESS",
            "MINA_ARCHIVE_GCP_STORAGE",
            "MINA_ARCHIVE_AWS_STORAGE",
            "MINA_FROM",
            "MINA_NO_FRONTIER_STORE",
        ] {
            assert!(names.contains(&name), "{name}");
        }
    }

    // CLI-REF: OCaml's daemon flag names, where meaning and values match ours
    // exactly, are visible aliases.
    #[test]
    fn ocaml_flag_names_are_visible_aliases() {
        let root = <MinaCli as clap::CommandFactory>::command();
        let node = find(&root, &["node"]);
        for (long, alias) in [
            ("libp2p-port", "external-port"),
            ("run-snarker", "run-snark-worker"),
            ("snarker-fee", "snark-worker-fee"),
            ("producer-key", "block-producer-key"),
            ("verbosity", "log-level"),
            ("peers", "peer"),
        ] {
            let arg = node
                .get_arguments()
                .find(|arg| arg.get_long() == Some(long))
                .unwrap();
            assert!(
                arg.get_visible_aliases()
                    .unwrap_or_default()
                    .contains(&alias),
                "{long}"
            );
        }
        // Not aliased: OCaml's `--work-selection` also takes `roffset`, and
        // its `--config-file` is repeatable and defaults to a file.
        for name in ["work-selection", "config-file"] {
            assert!(node.get_arguments().all(|arg| arg.get_long() != Some(name)
                && !arg.get_all_aliases().unwrap_or_default().contains(&name)));
        }
        let key = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg";
        let Command::Node(node) = parse(&[
            "node",
            "--external-port",
            "8312",
            "--run-snark-worker",
            key,
            "--snark-worker-fee",
            "0.25",
            "--block-producer-key",
            "/nonexistent/key",
        ])
        .unwrap()
        .command
        else {
            panic!("node");
        };
        assert_eq!(node.libp2p_port, 8312);
        assert_eq!(node.run_snarker.unwrap().to_string(), key);
        assert_eq!(node.snarker_fee, 250_000_000);
        assert_eq!(
            node.producer_key.unwrap(),
            std::path::PathBuf::from("/nonexistent/key")
        );
    }

    // CLI-REF: `keys` replaces `misc`, which stays as a hidden alias;
    // `frontier-store-check` moved under `node`; `mina-encrypted-key` is a
    // hidden alias of `wallet generate`; `snark` is gone.
    #[test]
    fn command_paths_and_hidden_aliases() {
        let root = <MinaCli as clap::CommandFactory>::command();
        let keys = find(&root, &["keys"]);
        assert!(!keys.is_hide_set());
        assert!(keys.get_visible_aliases().next().is_none());
        assert_eq!(keys.get_all_aliases().collect::<Vec<_>>(), ["misc"]);
        assert!(root.find_subcommand("snark").is_none());
        assert!(!find(&root, &["node", "frontier-store-check"]).is_hide_set());
        for hidden in ["frontier-store-check", "mina-encrypted-key"] {
            assert!(find(&root, &["keys", hidden]).is_hide_set(), "{hidden}");
        }
        for visible in ["mina-key-pair", "p2p-key-pair"] {
            assert!(!find(&root, &["keys", visible]).is_hide_set(), "{visible}");
        }
        // Both paths of each move parse to the same command.
        for args in [
            &["node", "frontier-store-check", "/s", "--verify", "root"][..],
            &["keys", "frontier-store-check", "/s", "--verify", "root"],
            &["misc", "frontier-store-check", "/s", "--verify", "root"],
            &["misc", "mina-key-pair"],
            &["misc", "p2p-key-pair"],
            &["wallet", "generate", "--output", "k"],
            &["misc", "mina-encrypted-key", "--file", "k"],
            &["misc", "mina-encrypted-key", "-f", "k"],
        ] {
            assert!(parse(args).is_ok(), "{args:?}");
        }
        assert!(parse(&["snark", "precalculate-block-verifier-index-and-srs"]).is_err());
        // Like `wallet generate`, the alias needs a file name.
        assert!(parse(&["misc", "mina-encrypted-key"]).is_err());
    }

    // CLI-REF: removed options are refused, not ignored.
    #[test]
    fn removed_options_are_refused() {
        for args in [
            &["node", "--experimental-catchup-rpc"][..],
            &["node", "--catchup-rpc-peer", "12D3KooW"],
            &[
                "wallet",
                "send",
                "--from",
                "k",
                "--to",
                "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg",
                "--amount",
                "1",
                "--fee",
                "0.1",
                "--fee-payer",
                "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg",
            ],
        ] {
            let error = parse(args).unwrap_err();
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::UnknownArgument,
                "{args:?}"
            );
        }
    }

    // CLI-REF: one visible form each turns the frontier store and the proof
    // cache off; the older forms are hidden.
    #[test]
    fn store_and_cache_switches_have_one_visible_form() {
        let root = <MinaCli as clap::CommandFactory>::command();
        let node = find(&root, &["node"]);
        let arg = |long: &str| {
            node.get_arguments()
                .find(|arg| arg.get_long() == Some(long))
                .unwrap()
        };
        assert!(!arg("no-frontier-store").is_hide_set());
        assert!(arg("frontier-store").is_hide_set());
        assert!(!arg("no-proof-cache").is_hide_set());
        assert!(!arg("proof-cache").is_hide_set());
        assert_eq!(
            arg("no-frontier-store").get_env(),
            Some(std::ffi::OsStr::new("MINA_NO_FRONTIER_STORE"))
        );
        assert_eq!(
            arg("no-proof-cache").get_env(),
            Some(std::ffi::OsStr::new("MINA_NO_PROOF_CACHE"))
        );
        // `--proof-cache` takes no value; MINA_PROOF_CACHE is read only as
        // a hidden fallback.
        assert_eq!(arg("proof-cache").get_env(), None);
        assert!(!arg("proof-cache").get_action().takes_values());
    }

    // CLI-REF: wallet amounts and fees are in MINA, as OCaml's.
    #[test]
    fn wallet_amounts_are_in_mina() {
        let to = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg";
        let send = |amount: &str, fee: &str| {
            parse(&[
                "wallet", "send", "--from", "k", "--to", to, "--amount", amount, "--fee", fee,
            ])
            .map(|cli| match cli.command {
                Command::Wallet(wallet::Wallet {
                    command: wallet::WalletCommand::Send(send),
                }) => (send.amount, send.fee),
                _ => panic!("send"),
            })
        };
        assert_eq!(send("2.5", "0.1").unwrap(), (2_500_000_000, 100_000_000));
        assert_eq!(send("1", "0.000000001").unwrap(), (1_000_000_000, 1));
        for (amount, fee) in [
            ("1.0000000001", "0.1"),
            ("-1", "0.1"),
            ("1", "-0.1"),
            ("18446744073.709551616", "0.1"),
            ("1", "1e9"),
        ] {
            let error = send(amount, fee).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        }
        let error = send("-1", "0.1").unwrap_err().to_string();
        assert!(error.contains("negative amounts are refused"), "{error}");
        let delegate = parse(&[
            "wallet", "delegate", "--from", "k", "--to", to, "--fee", "0.01",
        ]);
        let Command::Wallet(wallet::Wallet {
            command: wallet::WalletCommand::Delegate(delegate),
        }) = delegate.unwrap().command
        else {
            panic!("delegate");
        };
        assert_eq!(delegate.fee, 10_000_000);
        let root = <MinaCli as clap::CommandFactory>::command();
        for (path, long) in [
            (&["wallet", "send"][..], "amount"),
            (&["wallet", "send"], "fee"),
            (&["wallet", "delegate"], "fee"),
        ] {
            let arg = find(&root, path)
                .get_arguments()
                .find(|arg| arg.get_long() == Some(long))
                .unwrap();
            assert_eq!(arg.get_value_names().unwrap()[0].as_str(), "MINA");
        }
    }

    #[test]
    fn version_flag_uses_crate_version() {
        let error = <MinaCli as clap::Parser>::try_parse_from(["mina", "--version"])
            .expect_err("version flag exits after displaying the version");
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
        assert!(error.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn version_names_the_webrtc_backend() {
        let error = <MinaCli as clap::Parser>::try_parse_from(["mina", "--version"])
            .expect_err("version flag exits after displaying the version");
        let backend = mina_node::p2p::service_impl::webrtc::BACKEND;
        assert!(error.to_string().contains(&format!("(WebRTC: {backend})")));
        #[cfg(not(feature = "p2p-webrtc-rs"))]
        assert_eq!(backend, "rustrtc");
        #[cfg(feature = "p2p-webrtc-rs")]
        assert_eq!(backend, "webrtc-rs");
    }
}
