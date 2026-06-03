use std::io::{Read, Write};
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::json;

use crate::cli::{ChannelsCmd, DrainArgs, ListenArgs};
use crate::config::channels::ChannelConfig;

pub async fn run(cmd: ChannelsCmd) -> Result<()> {
    match cmd {
        ChannelsCmd::Listen(args) => listen(args).await,
        ChannelsCmd::Drain(args) => drain(args).await,
    }
}

async fn listen(args: ListenArgs) -> Result<()> {
    let workspace_root = std::env::current_dir().context("resolve current directory")?;
    let Some(config) = crate::channel::load_channels_config(&workspace_root) else {
        anyhow::bail!("no channels.yaml; configure first");
    };

    if args.poll {
        let channel = select_channel(&config, args.channel.as_deref())
            .context("select channel for polling")?;
        let entry = config
            .channels
            .get(&channel)
            .context("selected channel disappeared from config")?;
        let transport = crate::channel::select_transport(&channel, entry, &workspace_root)
            .context("select channel transport")?;
        loop {
            crate::channel::poll_inbound(
                &workspace_root,
                &channel,
                &config,
                transport.as_ref(),
                args.poll_limit,
            )
            .context("poll inbound channel messages")?;
            // Bound the wait on Ctrl+C so the loop exits cleanly after the
            // current iteration rather than the process aborting mid-sleep.
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break,
                _ = tokio::time::sleep(std::time::Duration::from_secs(args.interval_secs)) => {}
            }
        }
        Ok(())
    } else if args.once && args.from_stdin {
        let code = process_envelope(std::io::stdin().lock(), std::io::stdout().lock(), &config)?;
        if code == ExitCode::SUCCESS {
            Ok(())
        } else if code == ExitCode::from(2) {
            std::process::exit(2);
        } else {
            std::process::exit(1);
        }
    } else {
        anyhow::bail!("unsupported listen mode");
    }
}

async fn drain(args: DrainArgs) -> Result<()> {
    let workspace_root = std::env::current_dir().context("resolve current directory")?;
    let Some(config) = crate::channel::load_channels_config(&workspace_root) else {
        anyhow::bail!("no channels.yaml; configure first");
    };
    let channel = select_channel(&config, None).context("select channel for drain")?;
    let entry = config
        .channels
        .get(&channel)
        .context("selected channel disappeared from config")?;
    let transport = crate::channel::select_transport(&channel, entry, &workspace_root)
        .context("select channel transport")?;
    loop {
        crate::channel::drain_once(&args.run_dir, &config, transport.as_ref())
            .context("drain outbound channel replies")?;
        if args.once {
            return Ok(());
        }
        // Bound the wait on Ctrl+C so the loop exits cleanly after the current
        // iteration rather than the process aborting mid-sleep.
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(args.interval_secs)) => {}
        }
    }
}

fn select_channel(config: &ChannelConfig, requested: Option<&str>) -> Result<String> {
    if let Some(requested) = requested {
        anyhow::ensure!(
            config.channels.contains_key(requested),
            "channel `{requested}` not configured"
        );
        return Ok(requested.to_string());
    }
    config
        .channels
        .iter()
        .find_map(|(name, entry)| {
            (entry.enabled && entry.transport_session_id(name).is_some()).then_some(name.clone())
        })
        .context("no enabled channel with transport session")
}

pub fn process_envelope(
    mut reader: impl Read,
    mut writer: impl Write,
    config: &ChannelConfig,
) -> Result<ExitCode> {
    let mut raw = String::new();
    reader
        .read_to_string(&mut raw)
        .context("read channel message from stdin")?;
    let inbound: crate::channel::FeishuInbound = match serde_json::from_str(&raw) {
        Ok(inbound) => inbound,
        Err(err) => {
            write_parse_error(&mut writer, err.to_string())?;
            return Ok(ExitCode::from(2));
        }
    };
    let mut envelope = match crate::channel::parse_inbound(&inbound) {
        Ok(envelope) => envelope,
        Err(err) => {
            write_parse_error(&mut writer, err.to_string())?;
            return Ok(ExitCode::from(2));
        }
    };
    crate::channel::resolve_trust(&mut envelope, config);
    let decision = crate::channel::route_inbound(envelope, config);
    serde_json::to_writer(&mut writer, &decision).context("serialize route decision")?;
    writeln!(writer).context("write route decision newline")?;
    Ok(ExitCode::SUCCESS)
}

fn write_parse_error(mut writer: impl Write, error: String) -> Result<()> {
    serde_json::to_writer(&mut writer, &json!({ "error": error, "kind": "parse" }))
        .context("serialize parse error")?;
    writeln!(writer).context("write parse error newline")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::ExitCode;

    use chrono::{TimeZone, Utc};

    use crate::config::channels::{AllowedSender, ChannelConfig, ChannelDefaults, ChannelEntry};
    use crate::schema::channel_envelope::ChannelAction;

    #[test]
    fn listen_processes_inbound_and_prints_dispatch_decision() {
        let raw = serde_json::to_string(&inbound("plan", "ou_owner")).unwrap();
        let mut out = Vec::new();

        let code = super::process_envelope(raw.as_bytes(), &mut out, &config()).unwrap();

        assert_eq!(code, ExitCode::SUCCESS);
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let dispatch = value.get("Dispatch").expect("dispatch decision");
        assert_eq!(dispatch["action"], "plan");
        assert_eq!(dispatch["dry_run"], true);
        assert_eq!(dispatch["envelope"]["sender_trusted"], true);
    }

    #[test]
    fn listen_processes_inbound_and_prints_reject_decision() {
        let raw = serde_json::to_string(&inbound("run --run", "ou_untrusted")).unwrap();
        let mut out = Vec::new();

        let code = super::process_envelope(raw.as_bytes(), &mut out, &config()).unwrap();

        assert_eq!(code, ExitCode::SUCCESS);
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let reject = value.get("RejectWithReply").expect("reject decision");
        assert_eq!(reject["reason"], "untrusted_mutating");
        assert!(reject["reply_text"].as_str().unwrap().contains("untrusted"));
    }

    #[test]
    fn listen_exits_with_parse_error_when_inbound_malformed() {
        let mut out = Vec::new();

        let code = super::process_envelope(&b"not json"[..], &mut out, &config()).unwrap();

        assert_eq!(code, ExitCode::from(2));
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["kind"], "parse");
        assert!(value["error"].as_str().unwrap().contains("expected"));
    }

    #[test]
    fn select_channel_accepts_enabled_mock_without_botmux_session() {
        let mut config = config();
        config.channels.clear();
        config.channels.insert(
            "demo".to_string(),
            ChannelEntry {
                enabled: true,
                transport: "mock".to_string(),
                bot_open_id: None,
                botmux_session_id: None,
                allowed_senders: Vec::new(),
                allowed_actions: Vec::new(),
                reply_to_run_events: Vec::new(),
                redact_payload_over_kb: 1,
            },
        );

        let channel = super::select_channel(&config, None).unwrap();

        assert_eq!(channel, "demo");
    }

    fn config() -> ChannelConfig {
        let mut channels = BTreeMap::new();
        channels.insert(
            "feishu".to_string(),
            ChannelEntry {
                enabled: true,
                transport: "botmux".to_string(),
                bot_open_id: None,
                botmux_session_id: None,
                allowed_senders: vec![AllowedSender {
                    open_id: "ou_owner".to_string(),
                    label: None,
                }],
                allowed_actions: vec![
                    ChannelAction::Plan,
                    ChannelAction::Run,
                    ChannelAction::Approve,
                    ChannelAction::Status,
                ],
                reply_to_run_events: Vec::new(),
                redact_payload_over_kb: 1,
            },
        );
        ChannelConfig {
            version: 1,
            defaults: ChannelDefaults::default(),
            channels,
        }
    }

    fn inbound(body: &str, sender_open_id: &str) -> crate::channel::FeishuInbound {
        crate::channel::FeishuInbound {
            thread_id: Some("thread-1".to_string()),
            sender_open_id: sender_open_id.to_string(),
            message_id: "om_xxx".to_string(),
            create_time: Utc.with_ymd_and_hms(2026, 5, 24, 8, 0, 0).unwrap(),
            body: body.to_string(),
            attachments: Vec::new(),
        }
    }
}
