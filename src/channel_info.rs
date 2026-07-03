//! Channel information management for enhanced UUID completion

use anyhow::{Context, Result};
use freeswitch_esl_tokio::EslClient;
use serde::{Deserialize, Serialize};

/// Channel information from FreeSWITCH JSON output
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChannelInfo {
    pub uuid: String,
    pub created: String,
    pub created_epoch: String,
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub cid_name: String,
    #[serde(default)]
    pub cid_num: String,
}

/// Wrapper for FreeSWITCH JSON response
#[derive(Debug, Deserialize)]
pub struct ChannelsResponse {
    pub row_count: u32,
    pub rows: Vec<ChannelInfo>,
}

/// Channel information provider with smart fetching
pub struct ChannelProvider {
    max_channels: u32,
}

impl ChannelProvider {
    /// Create new channel provider with configurable limit
    pub fn new(max_channels: u32) -> Self {
        Self { max_channels }
    }

    /// Get enhanced UUID completions with channel info
    /// Returns formatted strings like: "uuid timestamp name (state)"
    /// Returns None if should fallback to default completion (too many channels)
    pub async fn get_uuid_completions(&self, client: &EslClient) -> Result<Option<Vec<String>>> {
        // First check channel count to avoid flooding
        let count = self
            .get_channel_count(client)
            .await?;

        if count == 0 {
            return Ok(Some(Vec::new()));
        }

        if count > self.max_channels {
            // Too many channels - fallback to default completion silently
            tracing::debug!(
                "Too many channels ({}) for enhanced completion, limit is {}. Falling back to default.",
                count, self.max_channels
            );
            return Ok(None);
        }

        // Fetch channel details
        let channels = self
            .get_channels(client)
            .await?;

        // Format for completion display
        let mut completions = Vec::new();
        for channel in channels {
            let formatted = format!(
                "{} {} {} ({})",
                channel.uuid, channel.created, channel.name, channel.state
            );
            completions.push(formatted);
        }

        Ok(Some(completions))
    }

    async fn fetch_channels_json(
        &self,
        client: &EslClient,
        command: &str,
    ) -> Result<ChannelsResponse> {
        let response = client
            .api(command)
            .await
            .with_context(|| format!("ESL API call '{}' failed", command))?;

        if !response.is_success() {
            anyhow::bail!(
                "ESL command '{}' returned: {}",
                command,
                response
                    .body()
                    .unwrap_or("-ERR")
            );
        }

        let body = response
            .body()
            .unwrap_or_default();
        serde_json::from_str::<ChannelsResponse>(body)
            .with_context(|| format!("Failed to parse JSON response for '{}'", command))
    }

    /// Get channel count using "show channels count as json"
    async fn get_channel_count(&self, client: &EslClient) -> Result<u32> {
        let resp = self
            .fetch_channels_json(client, "show channels count as json")
            .await?;
        Ok(resp.row_count)
    }

    /// Get channel details using "show channels as json"
    async fn get_channels(&self, client: &EslClient) -> Result<Vec<ChannelInfo>> {
        let resp = self
            .fetch_channels_json(client, "show channels as json")
            .await?;
        let mut channels = resp.rows;
        channels.sort_by(|a, b| {
            let a_epoch: u64 = a
                .created_epoch
                .parse()
                .unwrap_or(0);
            let b_epoch: u64 = b
                .created_epoch
                .parse()
                .unwrap_or(0);
            b_epoch.cmp(&a_epoch)
        });
        Ok(channels)
    }
}
