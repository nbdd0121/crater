mod api;
mod results;

use crate::agent::api::AgentApi;
use crate::agent::results::ResultsUploader;
use crate::config::Config;
use crate::crates::Crate;
use crate::experiments::Experiment;
use crate::prelude::*;
use crate::utils;
use failure::Error;
use rustwide::Workspace;
use std::ops;
use std::thread;
use std::time::Duration;

#[derive(Default, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    capabilities: Vec<String>,
}

impl ops::Deref for Capabilities {
    type Target = Vec<String>;

    fn deref(&self) -> &Self::Target {
        &self.capabilities
    }
}

impl ops::DerefMut for Capabilities {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.capabilities
    }
}

impl Capabilities {
    pub fn new(caps: &[&str]) -> Self {
        let capabilities = caps.iter().map(|s| s.to_string()).collect();
        Capabilities { capabilities }
    }
}

impl From<Vec<String>> for Capabilities {
    fn from(capabilities: Vec<String>) -> Self {
        Capabilities { capabilities }
    }
}

struct Agent {
    api: AgentApi,
    config: Config,
}

impl Agent {
    fn new(url: &str, token: &str, caps: &Capabilities) -> Fallible<Self> {
        info!("connecting to crater server {}...", url);

        let api = AgentApi::new(url, token);
        let config = api.config(caps)?;

        info!("connected to the crater server!");
        info!("assigned agent name: {}", config.agent_name);

        Ok(Agent {
            api,
            config: config.crater_config,
        })
    }

    fn experiment(&self) -> Fallible<(Experiment, Vec<Crate>)> {
        info!("asking the server for a new experiment...");
        Ok(self.api.next_experiment()?)
    }
}

fn run_heartbeat(url: &str, token: &str) {
    let api = AgentApi::new(url, token);

    thread::spawn(move || loop {
        if let Err(e) = api.heartbeat().with_context(|_| "failed to send heartbeat") {
            utils::report_failure(&e);
        }
        thread::sleep(Duration::from_secs(60));
    });
}

fn run_experiment(
    agent: &Agent,
    workspace: &Workspace,
    db: &ResultsUploader,
    threads_count: usize,
) -> Result<(), (Option<Experiment>, Error)> {
    let (ex, crates) = agent.experiment().map_err(|e| (None, e))?;
    crate::runner::run_ex(&ex, workspace, &crates, db, threads_count, &agent.config)
        .map_err(|err| (Some(ex), err))?;
    Ok(())
}

pub fn run(
    url: &str,
    token: &str,
    threads_count: usize,
    caps: &Capabilities,
    workspace: &Workspace,
) -> Fallible<()> {
    let agent = Agent::new(url, token, caps)?;
    let db = results::ResultsUploader::new(&agent.api);

    run_heartbeat(url, token);

    loop {
        if let Err((ex, err)) = run_experiment(&agent, workspace, &db, threads_count) {
            utils::report_failure(&err);
            if let Some(ex) = ex {
                if let Err(e) = agent
                    .api
                    .report_error(&ex, format!("{}", err.find_root_cause()))
                    .with_context(|_| "error encountered")
                {
                    utils::report_failure(&e);
                }
            }
        }
    }
}
