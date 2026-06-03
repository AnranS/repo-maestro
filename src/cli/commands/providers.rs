use anyhow::Result;

use crate::cli::ProvidersArgs;

pub fn run(args: ProvidersArgs) -> Result<()> {
    let providers = crate::providers::provider_statuses(args.adapters_only);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&providers)?);
        return Ok(());
    }

    println!(
        "{:<11} {:<11} {:<9} {:<10} {:<26} {:<8} {:<6} NOTES",
        "ID", "KIND", "ADAPTER", "INSTALLED", "BINARY", "TRACE", "NONINT"
    );
    for provider in providers {
        let binary = match (
            provider.execution.binary.as_deref(),
            provider.execution.path.as_deref(),
        ) {
            (Some(binary), Some(path)) => format!("{binary} -> {path}"),
            (Some(binary), None) => binary.to_string(),
            (None, _) => "(built-in)".to_string(),
        };
        println!(
            "{:<11} {:<11} {:<9} {:<10} {:<26} {:<8} {:<6} {}",
            provider.id,
            provider_kind_label(provider.kind),
            yes_no(provider.adapter_available),
            yes_no(provider.installed),
            truncate_cell(&binary, 26),
            trace_label(provider.tool_trace),
            yes_no(provider.execution.non_interactive),
            provider.notes
        );
        if let Some(env) = provider.execution.env_override {
            let value = std::env::var(env).unwrap_or_else(|_| "(unset)".into());
            println!(
                "{:<11} {:<11} {:<9} {:<10} {:<26} {env}: {value}",
                "", "", "", "", ""
            );
        }
    }
    println!("{}", providers_table_legend());
    Ok(())
}

pub(crate) fn provider_kind_label(kind: crate::providers::ProviderKind) -> &'static str {
    match kind {
        crate::providers::ProviderKind::TaskAdapter => "task",
        crate::providers::ProviderKind::KnownCli => "known-cli",
    }
}

fn trace_label(trace: crate::providers::ToolTraceSupport) -> &'static str {
    match trace {
        crate::providers::ToolTraceSupport::Full => "full",
        crate::providers::ToolTraceSupport::Partial => "partial",
        crate::providers::ToolTraceSupport::None => "none",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

pub(crate) fn providers_table_legend() -> &'static str {
    "legend: INSTALLED=binary present on PATH/env override, auth/session is not verified; TRACE=tool trace capture (full/partial/none); NONINT=yes means the CLI can run non-interactive tasks"
}

fn truncate_cell(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn providers_table_legend_explains_trace_and_noninteractive_columns() {
        let legend = super::providers_table_legend();

        assert!(legend.contains("TRACE"));
        assert!(legend.contains("NONINT"));
        assert!(legend.contains("non-interactive"));
        assert!(legend.contains("INSTALLED=binary present"));
        assert!(legend.contains("auth/session is not verified"));
    }
}
