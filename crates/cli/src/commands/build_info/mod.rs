use mina_node::BuildEnv;

/// Print this binary's version, commit and build details.
#[derive(Debug, clap::Args)]
pub struct Command;

impl Command {
    pub fn run(&self) -> anyhow::Result<()> {
        let build_env = BuildEnv::get();
        println!(
            r#"
Version:       {}
Build time:    {}
Commit SHA:    {}
Commit time:   {}
Commit branch: {}
Git describe:  {}
Rustc channel: {}
Rustc version: {}
WebRTC:        {}
"#,
            build_env.version,
            build_env.time,
            build_env.git.commit_hash,
            build_env.git.commit_time,
            build_env.git.branch,
            build_env.git.describe,
            build_env.rustc.channel,
            build_env.rustc.version,
            mina_node::p2p::service_impl::webrtc::BACKEND,
        );
        Ok(())
    }
}
