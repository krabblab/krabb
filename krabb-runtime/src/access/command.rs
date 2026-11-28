//! `krabb access`: shows and changes what each role may do, globally or in one place.

use std::path::Path;

use super::{Elevation, Grant, Preset, Role, Scope, layered};
use crate::{Config, PluginError};

#[derive(Debug, clap::Subcommand)]
pub(crate) enum Command {
    #[command(about = "Show each role's effective preset and admin level, globally and per place.")]
    List,
    #[command(
        about = "Give a role a preset, and optionally an admin level, globally or in one place."
    )]
    Set {
        #[arg(help = "`global`, or a channel or client ID such as telegram or tui.")]
        place: String,
        #[arg(help = "owner, admin, member, or guest.")]
        role: String,
        #[arg(help = "chat, hardened, standard, or loose.")]
        preset: String,
        #[arg(long, help = "none, packages, services, or full.")]
        elevation: Option<String>,
        #[arg(short, long, help = "Skip the confirmation for access above the role's default.")]
        yes: bool,
    },
}

/// Runs `krabb access list` or `krabb access set`.
pub(crate) fn run(command: Command, root: &Path, json: bool) -> Result<(), PluginError> {
    match command {
        Command::List => {
            let config = crate::load_config_at(root)?;
            let places = places(&config);

            if json {
                let value: Vec<_> = places
                    .iter()
                    .map(|(place, grants)| {
                        serde_json::json!({"place": place, "roles": grants
                        .iter()
                        .map(|grant| serde_json::json!({
                            "role": grant.role, "preset": grant.preset,
                            "elevation": grant.elevation, "visibility": grant.visibility,
                        }))
                        .collect::<Vec<_>>()})
                    })
                    .collect();

                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("{}", listing(&config, &places));
            }
        }

        Command::Set { place, role, preset, elevation, yes } => {
            let role = parse::<Role>(&role, "role", "owner, admin, member, or guest")?;
            let preset = parse::<Preset>(&preset, "preset", "chat, hardened, standard, or loose")?;

            let elevation = elevation
                .map(|value| {
                    parse::<Elevation>(&value, "admin level", "none, packages, services, or full")
                })
                .transpose()?;

            let config = crate::load_config_at(root)?;
            let key = key(&config, &place, role)?;

            if let Some(risk) = risk(role, preset, elevation) {
                let question = format!("{risk} Continue?");

                if !crate::agent::confirm_action(yes, json, &question)? {
                    crate::agent::print_cancelled("Access change", json);
                    return Ok(());
                }
            }

            let value: toml_edit::Value = match elevation {
                None => preset.name().into(),

                Some(elevation) => {
                    let mut table = toml_edit::InlineTable::new();

                    table.insert("preset", preset.name().into());
                    table.insert("elevation", elevation.name().into());
                    table.into()
                }
            };

            crate::save_setting(root, &key, value)?;

            match json {
                true => println!(
                    "{}",
                    serde_json::json!({"key": key, "preset": preset, "elevation": elevation})
                ),
                false => println!(
                    "Saved {key}. Restart the service to apply it: `krabb service restart`."
                ),
            }
        }
    }

    Ok(())
}

/// Every place with its own access settings, `global` first, each with the grant of every role.
fn places(config: &Config) -> Vec<(String, Vec<Grant>)> {
    let grants = |scope: Scope<'_>| {
        Role::ALL.iter().rev().map(|role| layered(config, *role, scope)).collect()
    };

    let mut places = vec![("global".to_owned(), grants(None))];

    for (id, channel) in &config.channels {
        places.push((format!("channels.{id}"), grants(Some((channel.preset, &channel.roles)))));
    }

    for (id, client) in &config.clients {
        places.push((format!("clients.{id}"), grants(Some((client.preset, &client.roles)))));
    }

    places
}

fn listing(config: &Config, places: &[(String, Vec<Grant>)]) -> String {
    let mut text = String::from("Access by role");

    if super::legacy(config) {
        text.push_str(
            "\nNo preset is set, so tools follow the older settings until one is.\nChoose one with \
             `krabb init preset`.",
        );
    }

    for (place, grants) in places {
        text.push_str(&format!("\n{place}"));

        for grant in grants {
            text.push_str(&format!(
                "\n  {:<6}  {:<8}  admin {:<8}  sees {}",
                grant.role.name(),
                grant.preset.name(),
                grant.elevation.name(),
                grant.visibility.name()
            ));
        }
    }

    text
}

/// The configuration key that holds ROLE's setting in PLACE.
fn key(config: &Config, place: &str, role: Role) -> Result<String, PluginError> {
    if place == "global" {
        return Ok(format!("roles.{role}"));
    }

    if config.channels.contains_key(place) {
        return Ok(format!("channels.{place}.roles.{role}"));
    }

    if config.clients.contains_key(place) || krabb_core::client::known(place) {
        return Ok(format!("clients.{place}.roles.{role}"));
    }

    Err(format!(
        "No channel or client is named {place}.\nUse `global` or a configured ID; list them with \
         `krabb access list`."
    )
    .into())
}

/// The one-line risk of giving ROLE more than its default, or none within it.
fn risk(role: Role, preset: Preset, elevation: Option<Elevation>) -> Option<String> {
    let elevation = elevation.unwrap_or(preset.elevation());

    if elevation == Elevation::Full {
        return Some(format!(
            "The {role} role gets full admin rights, so anyone in it can ask Krabb to change this \
             machine as root."
        ));
    }

    (preset > role.preset() || elevation > role.preset().elevation()).then(|| {
        format!("The {role} role gets more than its default ({}), at your risk.", role.preset())
    })
}

fn parse<T: serde::de::DeserializeOwned>(
    value: &str,
    what: &str,
    choices: &str,
) -> Result<T, PluginError> {
    use serde::de::IntoDeserializer;

    let parsed: Result<T, serde::de::value::Error> = T::deserialize(value.into_deserializer());

    parsed.map_err(|_| format!("Unknown {what} {value}, so use {choices}.").into())
}

#[cfg(test)]
mod tests;
