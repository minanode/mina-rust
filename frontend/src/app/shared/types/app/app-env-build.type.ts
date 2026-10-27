export interface AppEnvBuild {
  time: string;
  /** Product version of the node's build. */
  version?: string;
  git: GitEnvBuild;
  cargo: CargoEnvBuild;
  rustc: RustcEnvBuild;
}

interface GitEnvBuild {
  commit_time: string;
  commit_hash: string;
  branch: string;
  describe?: string;
}

interface CargoEnvBuild {
  features: string;
  opt_level: number;
  target: string;
  is_debug: boolean;
}

interface RustcEnvBuild {
  channel: string;
  commit_date: string;
  commit_hash: string;
  host: string;
  version: string;
  llvm_version: string;
}
