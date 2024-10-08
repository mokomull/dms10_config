use std::{fs::File, io::Write as _};

use anyhow::Context;
use log::{debug, info};

use crate::{console::Console, HASH};

pub struct Fetcher {
    filename: String,
    interactions: Vec<(String, String)>,
}

impl Fetcher {
    /// Create a Fetcher for the most common DMO interaction, e.g.
    ///
    ///         DMO000    CPK
    ///
    ///         REQ   que
    ///         TYP   pack
    ///         PACK  all
    ///
    /// That is, where the answer to `TYP` is exactly the same as the next prompt, and that prompt
    /// is predictably padded with spaces.
    pub fn common_dmo(ovly: &str, typ: &str) -> Self {
        Fetcher::common_dmo_with_prompt(ovly, typ, dmo_prompt(&typ.to_uppercase()))
    }

    /// Create a Fetcher another common DMO interaction, but with the last prompt customized.  This
    /// customization is either because the selection prompt isn't exactly the same as the answer to
    /// `TYP` (e.g. DNH vs. HGTP):
    ///
    ///         DMO000    HUNT
    ///
    ///         REQ   que
    ///         TYP   dnh
    ///         HTGP   all
    ///
    /// or because it is padded inconsistently with the most common DMS-10 pattern:
    ///
    ///         DMO000    NET
    ///
    ///         REQ   que
    ///         TYP   idt
    ///         IDT  all
    ///
    /// (i.e. one space too few)
    pub fn common_dmo_with_prompt(ovly: &str, typ: &str, prompt: impl Into<String>) -> Self {
        Self {
            filename: format!("{}/{}.txt", ovly.to_uppercase(), typ.to_uppercase()),
            interactions: vec![
                ("****\n".to_owned(), HASH.to_owned()),
                (format!("ovly {}\n", ovly), dmo_prompt("REQ")),
                ("que\n".to_owned(), dmo_prompt("TYP")),
                (format!("{}\n", typ), prompt.into()),
                ("all\n".to_owned(), dmo_prompt("REQ")),
            ],
        }
    }

    /// Create a Fetcher for some of the probably-newer overlays, which use a unique padding of
    /// *both* the `TYP` and the subsequent `MBS` or `LTG` prompt:
    ///
    ///         DMO000    MBS
    ///
    ///         REQ   que
    ///         TYP    mbs
    ///         MBS    all
    ///
    /// (i.e. one space too many)
    pub fn wide_dmo_with_prompt(ovly: &str, typ: &str, prompt: impl Into<String>) -> Self {
        Self {
            filename: format!("{}/{}.txt", ovly.to_uppercase(), typ.to_uppercase()),
            interactions: vec![
                ("****\n".to_owned(), HASH.to_owned()),
                (format!("ovly {}\n", ovly), dmo_prompt("REQ")),
                ("que\n".to_owned(), "    TYP    ".to_owned()),
                (format!("{}\n", typ), prompt.into()),
                ("all\n".to_owned(), dmo_prompt("REQ")),
            ],
        }
    }

    /// Create a Fetcher for interactions that don't ask the user for a selection -- i.e. they start
    /// spewing data immediately after `TYP` is answered:
    ///
    ///         DMO000    CNFG
    ///
    ///         REQ   que
    ///         TYP   cnfg
    pub fn common_dmo_no_prompt(ovly: &str, typ: &str) -> Self {
        Self {
            filename: format!("{}/{}.txt", ovly.to_uppercase(), typ.to_uppercase()),
            interactions: vec![
                ("****\n".to_owned(), HASH.to_owned()),
                (format!("ovly {}\n", ovly), dmo_prompt("REQ")),
                ("que\n".to_owned(), dmo_prompt("TYP")),
                (format!("{}\n", typ), dmo_prompt("REQ")),
            ],
        }
    }

    /// Create a Fetcher for the CLI overlay -- the answer to all `TYP` prompts is `cli`, and
    /// there's an additional `CLI` prompt that is the actual sub-configuration:
    ///
    ///         CLI000    CLI
    ///
    ///         REQ   que
    ///         TYP   cli
    ///         CLI   tg
    ///         TG    all
    ///
    /// This also supports a custom prompt, because for `CLI stn`, the subsequent `DN` prompt is
    /// padded differently than all the others.
    pub fn cli(ovly: &str, cli: &str, prompt: &str) -> Self {
        Self {
            filename: format!("{}/{}.txt", ovly.to_uppercase(), cli.to_uppercase()),
            interactions: vec![
                ("****\n".to_owned(), HASH.to_owned()),
                (format!("ovly {}\n", ovly), dmo_prompt("REQ")),
                ("que\n".to_owned(), dmo_prompt("TYP")),
                ("cli\n".to_owned(), dmo_prompt("CLI")),
                (format!("{}\n", cli), prompt.to_owned()),
                ("all\n".to_owned(), dmo_prompt("REQ")),
            ],
        }
    }

    /// Create a Fetcher for active translations:
    ///
    ///         DMO000    TRNS
    ///
    ///         REQ   que
    ///         TYP   ebsp
    ///         EBSP  all
    pub fn trns_active(typ: &str) -> Self {
        Self {
            filename: format!("TRNS/active/{}.txt", typ.to_uppercase()),
            interactions: vec![
                ("****\n".to_owned(), HASH.to_owned()),
                ("ovly trns\n".to_owned(), dmo_prompt("REQ")),
                ("que\n".to_owned(), dmo_prompt("TYP")),
                (format!("{}\n", typ), dmo_prompt(&typ.to_uppercase())),
                ("all\n".to_owned(), dmo_prompt("REQ")),
            ],
        }
    }

    /// Create a Fetcher for inactive translations (i.e. the `QUEI` request):
    ///
    ///         DMO000    TRNS
    ///
    ///         REQ   quei
    ///         TYP   ebsp
    ///         EBSP  all
    pub fn trns_inactive(typ: &str) -> Self {
        Self {
            filename: format!("TRNS/inactive/{}.txt", typ.to_uppercase()),
            interactions: vec![
                ("****\n".to_owned(), HASH.to_owned()),
                ("ovly trns\n".to_owned(), dmo_prompt("REQ")),
                ("quei\n".to_owned(), dmo_prompt("TYP")),
                (format!("{}\n", typ), dmo_prompt(&typ.to_uppercase())),
                ("all\n".to_owned(), dmo_prompt("REQ")),
            ],
        }
    }

    /// Fetch the configuration from the DMS-10, clean up whitespace and trailing prompts, and write
    /// it to a filename generated from its `OVLY` and `TYP`.
    pub async fn fetch_and_write(&self, console: &mut Console) -> anyhow::Result<()> {
        info!("fetching {}", self.filename);

        let buffer = self
            .fetch(console)
            .await
            .with_context(|| format!("fetching {}", self.filename))?;

        info!("finished fetching {}", self.filename);

        // strip CR at the beginning and end of each line, because they don't really help in the
        // non-terminal environment, and Github's web rendering chokes on it.
        //
        // This Vec is full of slices into buffer -- buffer is the variable that actually holds the
        // bytes.  Since buffer is immutable, we know we can't *change* any of the results of the//
        // DMS-10 command, we can merely choose to ignore the beginning/end of lines.
        let mut lines: Vec<&[u8]> = buffer.split(|&byte| byte == b'\n').collect();
        for line in &mut lines {
            let orig = *line; // just keep a record for the debug log.

            // stealing shamelessly from slice::trim_ascii_begin/end, because pattern matching makes
            // sense to my brain.  We're always allowed to *shrink* the slices into `buffer` :)
            while let [b'\r', rest @ ..] = line {
                *line = rest;
            }
            while let [rest @ .., b'\r'] = line {
                *line = rest;
            }

            debug!(
                "stripped \"{}\" into \"{}\"",
                orig.escape_ascii(),
                line.escape_ascii()
            );
        }

        // strip potentially some remaining data that was buffered from the previous command (in the
        // pipe to the subprocess, not in `buffer`), as well as the `****` to make the results more
        // similar to previous captures.  This assumes that the first thing we want to *keep* is the
        // line that starts with `  # `.
        if let Some(first_hash) = lines
            .iter()
            .position(|&line| line.starts_with(HASH.as_bytes()))
        {
            lines.drain(..first_hash);
        }

        // and strip the prompt that's printed out after the command completed
        if lines.len() >= 2
            && lines[lines.len() - 2] == b"    "
            && lines[lines.len() - 1] == b"    REQ   "
        {
            debug!("stripping blank line and REQ prompt");
            lines.drain((lines.len() - 2)..);
        }

        let mut file =
            File::create(&self.filename).with_context(|| format!("opening {}", self.filename))?;

        for line in lines {
            file.write_all(line)
                .and_then(|_| file.write_all(b"\n"))
                .with_context(|| format!("writing to {}", self.filename))?;
        }

        Ok(())
    }

    /// Get the filename that is generated from `OVLY` and `TYP`.  This can be used to uniquely
    /// identify the instance of Fetcher for the purposes of logging and filtering.
    pub fn filename(&self) -> &str {
        &self.filename
    }

    async fn fetch(&self, console: &mut Console) -> anyhow::Result<Vec<u8>> {
        let mut output = vec![];

        for (send, expect) in &self.interactions {
            console
                .send(send.as_bytes())
                .await
                .with_context(|| format!("sending {}", send.as_bytes().escape_ascii()))?;
            let mut buffer = console
                .run_until_human_prompt(expect)
                .await
                .with_context(|| format!("waiting for {}", expect.as_bytes().escape_ascii()))?;
            output.append(&mut buffer);
        }

        Ok(output)
    }
}

fn dmo_prompt(prompt: &str) -> String {
    format!("    {:4}  ", prompt)
}
