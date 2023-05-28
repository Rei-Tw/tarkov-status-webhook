use std::{collections::HashMap, fmt};

use anyhow::Result;
use chrono::{DateTime, Utc};
use reqwest::{self, header::AUTHORIZATION};
use serde::Deserialize;
use serde_repr::Deserialize_repr;
use tokio::time;
use webhook::client::WebhookClient;

#[macro_use]
extern crate log;

#[derive(Deserialize_repr, Debug, Clone)]
#[repr(u32)]
enum EventType {
    #[serde(other)]
    Unknown = 0,
    UpdateInstallation = 1,
    ServerIssues = 2,
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EventType::UpdateInstallation => write!(f, "Installation de mise à jour"),
            EventType::ServerIssues => write!(f, "Problèmes de serveur"),
            _ => write!(f, "Inconnu"),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
struct Event {
    #[serde(alias = "_id")]
    id: String,
    content: String,
    #[serde(alias = "type")]
    event_type: EventType,
    time: DateTime<Utc>,
    #[serde(alias = "solveTime")]
    solve_time: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Debug)]
struct Translation {
    text: String,
}

#[derive(Deserialize, Debug)]
struct DeeplResponse {
    translations: Vec<Translation>,
}

// todo: do this better :v)
const DEEPL_API_KEY: &'static str = "API_KEY";

// This function will attempt to translate. On fail it'll just return the same content, untranslated.
async fn try_translate(reqwest_client: &reqwest::Client, text: &String) -> String {
    let params = [("text", text.as_str()), ("target_lang", "FR")];

    match reqwest_client
        .post("https://api-free.deepl.com/v2/translate")
        .form(&params)
        .header(AUTHORIZATION, format!("DeepL-Auth-Key {DEEPL_API_KEY}"))
        .send()
        .await
    {
        Ok(resp) => match resp.error_for_status() {
            Ok(resp) => {
                let deepl_resp: DeeplResponse = resp.json().await.unwrap();
                if deepl_resp.translations.len() > 0 {
                    return deepl_resp.translations.get(0).unwrap().text.clone();
                }
            }
            Err(e) => error!("Deepl API returned error: {e}"),
        },
        Err(e) => error!("Unexpected error has occured while contacting Deepl API: {e}"),
    }

    text.clone()
}

// todo: do this better :v)
const WEBHOOK_URL: &'static str = "url";

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let mut interval = time::interval(std::time::Duration::from_secs(30));

    let reqwest_client = reqwest::Client::new();
    let webhook_client = WebhookClient::new(WEBHOOK_URL);

    let mut saved_events: HashMap<String, Event> = HashMap::new();

    loop {
        interval.tick().await;

        let events: Vec<Event> = match reqwest_client
            .get("https://status.escapefromtarkov.com/api/message/list")
            .send()
            .await
        {
            Ok(resp) => match resp.error_for_status() {
                Ok(resp) => resp.json().await.unwrap(),
                Err(e) => {
                    error!("Api status.escapefromtarkov.com returned error: {e}");
                    Vec::new()
                }
            },
            Err(e) => {
                error!("Unexpected error has occured while contacting status.escapefromtarkov.com: {e}");
                Vec::new()
            }
        };

        for event in events.iter() {
            if let Some(saved_event) = saved_events.get(&event.id) {
                if saved_event.solve_time != None {
                    continue;
                }
            }

            let translated_content = try_translate(&reqwest_client, &event.content).await;

            let resp = webhook_client
                .send(|message: &mut webhook::models::Message| {
                    message
                        .username("Escape from Tarkov Status")
                        .embed(|embed| {
                            // Global settings for the embed
                            embed
                                .title(event.event_type.to_string().as_str())
                                .thumbnail(
                                    "https://www.escapefromtarkov.com/themes/eft/images/logo.png",
                                )
                                .description(translated_content.as_str())
                                .url("https://status.escapefromtarkov.com");

                            // tweak some params if solved
                            if let Some(solve_time) = event.solve_time {
                                embed
                                    .field(
                                        "Résolu depuis",
                                        format!("<t:{}:R>", solve_time.timestamp()).as_str(),
                                        true,
                                    )
                                    .color("65280");

                                embed.field("Status", "Résolu :white_check_mark:", false);

                            // or not
                            } else {
                                embed
                                    .field(
                                        "Depuis",
                                        format!("<t:{}:R>", event.time.timestamp()).as_str(),
                                        true,
                                    )
                                    .color("16711680");

                                embed.field(
                                    "Status",
                                    "Hors ligne :negative_squared_cross_mark:",
                                    false,
                                );
                            }

                            embed
                        })
                })
                .await;

            if let Err(e) = resp {
                error!("Failed to send message to Discord webhook: {e}")
            }

            saved_events.insert(event.id.clone(), event.clone());
        }

        // cleanup old events
        saved_events.retain(|k, _| events.iter().any(|e| e.id == *k));
    }
}
