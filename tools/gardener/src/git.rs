use crate::errors::GardenerError;
use crate::runtime::{ProcessRequest, ProcessRunner};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeMode {
    MergeableOnly,
    MergeToMain,
}

pub struct GitClient<'a> {
    runner: &'a dyn ProcessRunner,
    cwd: PathBuf,
}

impl<'a> GitClient<'a> {
    pub fn new(runner: &'a dyn ProcessRunner, cwd: impl AsRef<Path>) -> Self {
        Self {
            runner,
            cwd: cwd.as_ref().to_path_buf(),
        }
    }

    pub fn detect_detached_head(&self) -> Result<bool, GardenerError> {
        let out = self.run(["git", "symbolic-ref", "--short", "HEAD"])?;
        Ok(out.exit_code != 0)
    }

    pub fn verify_ancestor(
        &self,
        maybe_ancestor: &str,
        branch: &str,
    ) -> Result<bool, GardenerError> {
        let out = self.run(["git", "merge-base", "--is-ancestor", maybe_ancestor, branch])?;
        Ok(out.exit_code == 0)
    }

    pub fn push_with_rebase_recovery(&self, branch: &str) -> Result<(), GardenerError> {
        let first = self.run(["git", "push", "origin", branch])?;
        if first.exit_code == 0 {
            return Ok(());
        }

        let rebase = self.run(["git", "pull", "--rebase", "origin", branch])?;
        if rebase.exit_code != 0 {
            return Err(GardenerError::Process(
                "push/rebase recovery failed".to_string(),
            ));
        }

        let second = self.run(["git", "push", "origin", branch])?;
        if second.exit_code != 0 {
            return Err(GardenerError::Process(
                "push failed after rebase recovery".to_string(),
            ));
        }

        Ok(())
    }

    pub fn run_validation_command(&self, command: &str) -> Result<(), GardenerError> {
        let out = self.run(["sh", "-lc", command])?;
        if out.exit_code != 0 {
            return Err(GardenerError::Process(
                "post-merge validation command failed".to_string(),
            ));
        }
        Ok(())
    }

    fn run<I, S>(&self, args: I) -> Result<crate::runtime::ProcessOutput, GardenerError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let parts = args
            .into_iter()
            .map(|s| s.as_ref().to_string())
            .collect::<Vec<_>>();
        let program = parts.first().cloned().unwrap_or_default();
        let argv = parts.iter().skip(1).cloned().collect::<Vec<_>>();
        self.runner.run(ProcessRequest {
            program,
            args: argv,
            cwd: Some(self.cwd.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::GitClient;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};

    #[test]
    fn push_rebase_recovery_path() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "push failed".to_string(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));

        GitClient::new(&runner, "/repo")
            .push_with_rebase_recovery("feature/x")
            .expect("recovered");
    }

    #[test]
    fn detached_head_detection() {
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let detached = GitClient::new(&runner, "/repo")
            .detect_detached_head()
            .expect("checked");
        assert!(detached);
    }
}
