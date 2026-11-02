use super::{ImageProfile, ImageProvider, valid_description, valid_lora_file, wangp::Wangp};
use anyhow::{Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::HashSet, time::Duration};

#[derive(Serialize)]
pub struct ImageInventory {
    profile_id: String,
    model: String,
    available: bool,
    capabilities: Vec<String>,
    models: Vec<ImageModel>,
    loras: Vec<String>,
}

#[derive(Serialize)]
struct ImageModel {
    id: String,
    label: String,
}

// All calls are fixed read operations. No workspace creation, queue mutation,
// model loading, settings import or generation occurs on this path.
pub async fn inspect(profile: &ImageProfile) -> Result<ImageInventory> {
    ensure!(
        profile.provider == ImageProvider::Wangp,
        "This preset has no WanGP inventory."
    );
    let mut client =
        tokio::time::timeout(Duration::from_secs(10), Wangp::connect(&profile.endpoint)).await??;
    let result = tokio::time::timeout(Duration::from_secs(15), read(&mut client, profile)).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), client.close()).await;
    result?
}

async fn collection(
    client: &mut Wangp,
    tool: &str,
    mut args: Value,
    key: &str,
) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    let mut cursors = HashSet::new();
    for _ in 0..16 {
        let value = client.tool(tool, args.clone()).await?;
        let page = value[key]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid WanGP inventory page."))?;
        ensure!(
            items.len() + page.len() <= 256,
            "WanGP inventory exceeds 256 entries; use its native interface."
        );
        items.extend(page.iter().cloned());
        match value["has_more"].as_bool() {
            Some(false) => return Ok(items),
            Some(true) => {
                let cursor = value["next_cursor"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 1024)
                    .ok_or_else(|| anyhow::anyhow!("Invalid WanGP inventory cursor."))?;
                ensure!(
                    cursors.insert(cursor.to_owned()),
                    "Repeated WanGP inventory cursor."
                );
                args["arguments"]["cursor"] = json!(cursor);
            }
            None => anyhow::bail!("Missing WanGP inventory continuation status."),
        }
    }
    anyhow::bail!("WanGP inventory exceeds the page limit.")
}

async fn read(client: &mut Wangp, profile: &ImageProfile) -> Result<ImageInventory> {
    let model = profile.info().model;
    let rows = collection(
        client,
        "wangp_models",
        json!({"action":"search","arguments":{"filters":{"main_output":"image"},"limit":50}}),
        "models",
    )
    .await?;
    let mut ids = HashSet::new();
    let mut models = Vec::new();
    for row in rows {
        let id = row["model_type"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
            .ok_or_else(|| anyhow::anyhow!("Invalid image model ID."))?;
        let label = row["name"]
            .as_str()
            .filter(|s| valid_description(s, 256))
            .ok_or_else(|| anyhow::anyhow!("Invalid image model label."))?;
        ensure!(ids.insert(id.to_owned()), "Duplicate image model ID.");
        models.push(ImageModel {
            id: id.to_owned(),
            label: label.to_owned(),
        });
    }
    ensure!(
        ids.contains(&model),
        "The configured image model was not found in WanGP's image inventory."
    );
    let caps = client
        .tool(
            "wangp_model",
            json!({"model_type":model,"action":"capabilities","arguments":{}}),
        )
        .await?;
    ensure!(
        caps["metadata"]["model_type"] == model,
        "Image capability identity mismatch."
    );
    let capabilities = caps["metadata"]["capabilities"]
        .as_array()
        .filter(|a| a.len() <= 64)
        .ok_or_else(|| anyhow::anyhow!("Invalid image capabilities."))?
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| valid_description(s, 128))
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("Invalid image capability."))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut loras = Vec::new();
    let mut seen = HashSet::new();
    if capabilities.iter().any(|c| c == "lora") {
        for value in collection(
            client,
            "wangp_model",
            json!({"model_type":model,"action":"loras","arguments":{"limit":50}}),
            "loras",
        )
        .await?
        {
            let file = value
                .as_str()
                .filter(|s| valid_lora_file(s))
                .ok_or_else(|| anyhow::anyhow!("WanGP returned an unsupported LoRA identifier."))?;
            ensure!(seen.insert(file.to_owned()), "Duplicate LoRA identifier.");
            loras.push(file.to_owned());
        }
    }
    let settings = profile.settings_for("Inspect local checkpoint availability", 0, &[])?;
    let check = client
        .tool("wangp_validate", json!({"settings":settings}))
        .await?;
    let tasks = check["tasks"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or_else(|| anyhow::anyhow!("Invalid availability response."))?;
    ensure!(
        check["submitted"] == false
            && tasks[0]["model_type"] == model
            && tasks[0]["availability"]["model_type"] == model,
        "Image availability identity mismatch."
    );
    let available = tasks[0]["availability"]["available"]
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("Missing checkpoint availability."))?;
    Ok(ImageInventory {
        profile_id: profile.id.clone(),
        model,
        available,
        capabilities,
        models,
        loras,
    })
}
