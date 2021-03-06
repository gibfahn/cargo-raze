// Copyright 2018 Google Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
  fs::{self, File},
  io::Write,
  path::PathBuf,
};

use anyhow::Result;

use docopt::Docopt;

use cargo_raze::{
  bazel::{find_workspace_root, BazelRenderer},
  metadata::{CargoMetadataFetcher, CargoWorkspaceFiles, MetadataFetcher},
  planning::{BuildPlanner, BuildPlannerImpl},
  rendering::{BuildRenderer, FileOutputs, RenderDetails},
  settings::{load_settings, GenMode},
  util::PlatformDetails,
};

use serde_derive::Deserialize;

#[derive(Debug, Deserialize)]
struct Options {
  arg_buildprefix: Option<String>,
  flag_verbose: u32,
  flag_quiet: Option<bool>,
  flag_host: Option<String>,
  flag_color: Option<String>,
  flag_target: Option<String>,
  flag_dryrun: Option<bool>,
  flag_cargo_bin_path: Option<String>,
  flag_output: String,
}

const USAGE: &str = r#"
Generate BUILD files for your pre-vendored Cargo dependencies.

Usage:
    cargo raze (-h | --help)
    cargo raze [--verbose] [--quiet] [--color=<WHEN>] [--dryrun] [--cargo-bin-path=<PATH>] [--output=<PATH>]
    cargo raze <buildprefix> [--verbose] [--quiet] [--color=<WHEN>] [--dryrun] [--cargo-bin-path=<PATH>]
                             [--output=<PATH>]

Options:
    -h, --help                         Print this message
    -v, --verbose                      Use verbose output
    -q, --quiet                        No output printed to stdout
    --color=<WHEN>                     Coloring: auto, always, never
    -d, --dryrun                       Do not emit any files
    --cargo-bin-path=<PATH>            Path to the cargo binary to be used for loading workspace metadata
    --output=<PATH>                    Path to output the generated into.
"#;

fn main() -> Result<()> {
  let options: Options = Docopt::new(USAGE)
    .and_then(|d| d.deserialize())
    .unwrap_or_else(|e| e.exit());

  let settings = load_settings("Cargo.toml")?;
  println!("Loaded override settings: {:#?}", settings);

  let mut metadata_fetcher: Box<dyn MetadataFetcher> = match options.flag_cargo_bin_path {
    Some(ref p) => Box::new(CargoMetadataFetcher::new(p)),
    None => Box::new(CargoMetadataFetcher::default()),
  };
  let mut planner = BuildPlannerImpl::new(&mut *metadata_fetcher);

  let toml_path = PathBuf::from("./Cargo.toml");
  let lock_path_opt = fs::metadata("./Cargo.lock")
    .ok()
    .map(|_| PathBuf::from("./Cargo.lock"));
  let files = CargoWorkspaceFiles {
    toml_path,
    lock_path_opt,
  };
  let platform_details = PlatformDetails::new_using_rustc(&settings.target)?;
  let planned_build = planner.plan_build(&settings, files, platform_details)?;
  let mut bazel_renderer = BazelRenderer::new();

  // Default to the current directory '.'
  let mut prefix_path: PathBuf = PathBuf::new();
  prefix_path.push(".");

  // Allow the command line option to take precedence
  if options.flag_output.is_empty() {
    if settings.incompatible_relative_workspace_path {
      if let Some(workspace_root) = find_workspace_root() {
        prefix_path.clear();
        prefix_path.push(workspace_root);
        prefix_path.push(
          &planned_build
            .workspace_context
            .workspace_path
            // Remove the leading "//" from the path
            .trim_start_matches('/'),
        );
      }
    }
  } else {
    prefix_path.clear();
    prefix_path.push(options.flag_output);
  }

  let render_details = RenderDetails {
    path_prefix: prefix_path.display().to_string(),
    buildfile_suffix: settings.output_buildfile_suffix,
  };

  let dry_run = options.flag_dryrun.unwrap_or(false);
  if !dry_run {
    fs::create_dir_all(&render_details.path_prefix)?;
  }

  let bazel_file_outputs = match settings.genmode {
    GenMode::Vendored => bazel_renderer.render_planned_build(&render_details, &planned_build)?,
    GenMode::Remote => {
      if !dry_run {
        // Create "remote/" if it doesn't exist
        fs::create_dir_all(render_details.path_prefix.clone() + "/remote/")?;
      }

      bazel_renderer.render_remote_planned_build(&render_details, &planned_build)?
    }, /* exhaustive, we control the definition */
  };

  for FileOutputs {
    path,
    contents,
  } in bazel_file_outputs
  {
    if dry_run {
      println!("{}:\n{}", path, contents);
    } else {
      write_to_file_loudly(&path, &contents)?;
    }
  }

  Ok(())
}

fn write_to_file_loudly(path: &str, contents: &str) -> Result<()> {
  File::create(&path).and_then(|mut f| f.write_all(contents.as_bytes()))?;
  println!("Generated {} successfully", path);
  Ok(())
}
