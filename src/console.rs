use std::{cmp::min, process::Stdio, time::Duration};

use anyhow::Context;
use log::{debug, warn};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{ChildStdin, ChildStdout, Command},
};

// Warn that maybe the DMS-10 console is stuck since we haven't gotten to a human prompt in this
// amount of time.
const TIMEOUT: Duration = Duration::from_secs(5);
// number of bytes to include in these warnings
const LOOKBACK: usize = 100;

pub struct Console {
    stdin: ChildStdin,
    stdout: ChildStdout,
    buffer: Vec<u8>,
}

impl Console {
    pub async fn new(hostname: &str) -> anyhow::Result<Self> {
        let child = Command::new("/usr/bin/telnet")
            .arg(hostname)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // the telnet process traps SIGINT and does special behavior with it, so make sure it is
            // not in the *foreground* process group.  This way it can keep running while we handle
            // ctrl-C in main.rs.
            .process_group(0)
            .spawn()
            .context("spawning /usr/bin/telnet")?;

        let stdin = child.stdin.expect("stdin should have been piped");
        let stdout = child.stdout.expect("stdout should have been piped");

        Ok(Self {
            stdin,
            stdout,
            buffer: vec![],
        })
    }

    pub async fn run_until_human_prompt(
        &mut self,
        expected_prompt: &str,
    ) -> anyhow::Result<Vec<u8>> {
        loop {
            match tokio::time::timeout(TIMEOUT, self.read_until_prompt(expected_prompt)).await {
                Err(_elapsed) => {
                    warn!(
                        "DMS-10 has not reached the expected prompt \"{}\", tail of the buffer is: \"{}\"",
                        expected_prompt.as_bytes().escape_ascii(),
                        self.buffer[(self.buffer.len() - (min(self.buffer.len(), LOOKBACK)))..]
                            .escape_ascii()
                    )
                }
                Ok(result) => match result {
                    Ok(()) => return Ok(std::mem::take(&mut self.buffer)),
                    Err(e) => return Err(e).context("reading from socket"),
                },
            }
        }
    }

    pub async fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        debug!("sending: {}", data.escape_ascii());
        self.stdin
            .write_all(data)
            .await
            .context("writing to child")?;

        // pretend we're echoing all typed words to the screen, so pre-load the buffer with the data
        // we just sent.
        self.buffer.extend_from_slice(data);

        Ok(())
    }

    async fn read_until_prompt(&mut self, expected_prompt: &str) -> anyhow::Result<()> {
        loop {
            self.read_into_buffer().await?;

            if self.check_buffer_tail(expected_prompt) {
                return Ok(());
            }
        }
    }

    // this is basically stream.read_buf()'ing into self.buffer, *except* that I get to
    // debug-log every time it actually gets bytes
    async fn read_into_buffer(&mut self) -> anyhow::Result<()> {
        let mut new_buf = vec![];
        let count = self.stdout.read_buf(&mut new_buf).await?;
        if count == 0 {
            debug!("EOF!");
            anyhow::bail!("subprocess returned EOF");
        }
        debug!("received: \"{}\"", new_buf.escape_ascii());
        self.buffer.append(&mut new_buf);

        Ok(())
    }

    fn check_buffer_tail(&mut self, expected_prompt: &str) -> bool {
        for newline in [b'\n', b'\r'] {
            let mut needle = vec![newline];
            needle.extend_from_slice(expected_prompt.as_bytes());
            if self.buffer.ends_with(&needle) {
                return true;
            }
        }

        false
    }
}
