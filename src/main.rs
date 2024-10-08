use std::{collections::HashSet, time::Duration};

use anyhow::Context;
use clap::Parser;
use console::Console;
use fetcher::Fetcher;
use log::{debug, info, warn};
use tokio::select;

mod console;
mod fetcher;

static HASH: &str = "  # ";

#[derive(Debug, clap::Parser)]
struct Config {
    #[arg(long, default_value = "10.27.20.179")]
    hostname: String,

    #[arg(skip)]
    password: String,

    #[arg(
        help = "resources to fetch from the DMS-10.  Specify the target filename, e.g. NET/DSLK.txt"
    )]
    files: Vec<String>,
}

impl Config {
    fn read_password(mut self) -> Self {
        self.password = if let Ok(x) = std::env::var("DMS10_PASSWORD") {
            x
        } else if let Ok(x) = rpassword::prompt_password("Password: ") {
            x
        } else {
            panic!("Please provide the DMS10 password.");
        };
        self
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();

    let config = Config::parse().read_password();
    debug!("parsed configuration: {:?}", config);

    let mut console = Console::new(&config.hostname).await?;
    info!("connected to DMS-10!");

    console.run_until_human_prompt("user: ").await?;

    console.send(b"root\n").await.context("sending username")?;

    console.run_until_human_prompt("password: ").await?;

    let mut password_buffer = config.password;
    password_buffer.push('\n');
    console
        .send(password_buffer.as_bytes())
        .await
        .context("sending password")?;

    console.run_until_human_prompt(" $ ").await?;

    console
        .send(b"dmstty 21\n")
        .await
        .context("choosing a LOGU")?;
    // blindly wait one second before sending **** to get a logged-out prompt
    tokio::time::sleep(Duration::from_secs(1)).await;
    console.send(b"****\n").await.context("****")?;

    console.run_until_human_prompt("  ! ").await?;

    console.send(b"logi\n").await.context("LOGI")?;
    console.run_until_human_prompt("    PASS? ").await?;

    console
        .send(password_buffer.as_bytes())
        .await
        .context("sending password (DMS-10)")?;
    console.run_until_human_prompt(HASH).await?;

    let common: &[(&str, &[&str])] = &[
        ("alrm", &["alpt"]),
        ("area", &["hnpa", "rc"]),
        ("cpk", &["dcm", "idtl", "lpk", "pack", "slc", "slpk"]),
        ("lan", &["lac", "lshf"]),
        (
            "net",
            &["d1pk", "ds1l", "dsi", "dslk", "edch", "esma", "ifpk", "scs"],
        ),
        ("snet", &["snls"]),
        ("thgp", &["thgp"]),
        ("trk", &["dtrk", "ltrk", "trk"]),
    ];

    let mut fetchers = vec![
        Fetcher::common_dmo_with_prompt("hunt", "dnh", "    HTGP   "),
        Fetcher::common_dmo_with_prompt("hunt", "ebs", "    EBSG   "),
        Fetcher::common_dmo_no_prompt("ain", "adsc"),
        Fetcher::common_dmo_with_prompt("ain", "lnp", "    LNP1  "),
        Fetcher::common_dmo_no_prompt("ain", "slhr"),
        Fetcher::common_dmo_with_prompt("ama", "ama", "    CTYP  "),
        Fetcher::common_dmo_no_prompt("area", "lrn"),
        Fetcher::cli("cli", "ltg", "    LTG   "),
        Fetcher::cli("cli", "stn", "    DN   "),
        Fetcher::cli("cli", "tg", "    TG    "),
        Fetcher::common_dmo_no_prompt("cnfg", "cnfg"),
        Fetcher::common_dmo_with_prompt("dn", "dn", "    DN   "),
        Fetcher::common_dmo_with_prompt("dn", "stn", "    DN   "),
        Fetcher::common_dmo_no_prompt("lan", "lci"),
        Fetcher::wide_dmo_with_prompt("mbs", "mbs", "    MBS    "),
        Fetcher::common_dmo_with_prompt("net", "idt", "    IDT  "),
        Fetcher::wide_dmo_with_prompt("pri", "pri", "    LTG    "),
        Fetcher::common_dmo_with_prompt("rout", "brte", "    BRTE   "),
        Fetcher::common_dmo_with_prompt("rout", "dest", "    DEST   "),
        Fetcher::common_dmo_with_prompt("rout", "rout", "    ROUT   "),
        Fetcher::common_dmo_with_prompt("snet", "snl", "    SNLS  "),
        Fetcher::common_dmo_with_prompt("snet", "snrs", "    LEVL  "),
        Fetcher::wide_dmo_with_prompt("tg", "ltg", "    NUM    "),
        Fetcher::wide_dmo_with_prompt("tg", "tg", "    NUM    "),
        Fetcher::trns_active("dns"),
    ];

    for &(ovly, typs) in common {
        for &typ in typs {
            fetchers.push(Fetcher::common_dmo(ovly, typ));
        }
    }

    for typ in ["addr", "ebsp", "prfx", "scrn"] {
        fetchers.push(Fetcher::trns_active(typ));
        fetchers.push(Fetcher::trns_inactive(typ));
    }

    fetchers.sort_unstable_by(|x, y| x.filename().cmp(y.filename()));

    let files: HashSet<String> = config.files.into_iter().collect();

    'next_fetcher: for fetcher in fetchers {
        'repeat_this_fetcher: loop {
            if !files.is_empty() && !files.contains(fetcher.filename()) {
                // the user passed in a filter list, and this fetcher is not in it.  skip it entirely.
                continue 'next_fetcher;
            }

            let fetch_future = fetcher.fetch_and_write(&mut console);
            tokio::pin!(fetch_future);

            'keep_waiting: loop {
                let ctrl_c = tokio::signal::ctrl_c();

                select! {
                    r = &mut fetch_future => {
                        r.with_context(|| format!("fetch_and_write {}", fetcher.filename()))?;
                        continue 'next_fetcher;
                    }
                    _ = ctrl_c => {
                        // TODO: this will potentially process data that has been buffered during a
                        // long-running fetch, if the user accidentally typed something on their
                        // keyboard.  Ideally we could clear the stdin buffer before doing this, but
                        // ... there is no try_read_all() or something that will read up until it
                        // *blocks* rather than EOF.
                        let stdin = std::io::stdin();
                        loop {
                            eprintln!("Ctrl-C detected.  Say 'w' to keep waiting, 'r' to repeat this OVLY and TYP, or 'n' to skip to the next.");

                            let mut buf = String::new();
                            stdin.read_line(&mut buf).context("reading from stdin failed")?;
                            match buf.as_str().trim_ascii_end() {
                                "w" => continue 'keep_waiting, // TODO: this seems not to work...?
                                "r" => continue 'repeat_this_fetcher,
                                "n" => continue 'next_fetcher,
                                _ => eprintln!("That was not one of the options, try again."),
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
