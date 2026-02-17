//! Channel information management for enhanced UUID completion

use anyhow::Result;
use freeswitch_esl_rs::EslClient;
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

    /// Get channel count using "show channels count as json"
    async fn get_channel_count(&self, client: &EslClient) -> Result<u32> {
        let response = client
            .api("show channels count as json")
            .await?;

        if !response.is_success() {
            return Ok(0);
        }

        let body = response.body_string();
        match serde_json::from_str::<ChannelsResponse>(&body) {
            Ok(channels_response) => Ok(channels_response.row_count),
            Err(_) => Ok(0), // Parse error, assume no channels
        }
    }

    /// Get channel details using "show channels as json"
    async fn get_channels(&self, client: &EslClient) -> Result<Vec<ChannelInfo>> {
        let response = client
            .api("show channels as json")
            .await?;

        if !response.is_success() {
            return Ok(Vec::new());
        }

        let body = response.body_string();
        match serde_json::from_str::<ChannelsResponse>(&body) {
            Ok(channels_response) => {
                let mut channels = channels_response.rows;
                // Sort by created_epoch (newest first)
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
            Err(e) => {
                tracing::debug!("Failed to parse channels JSON: {}", e);
                Ok(Vec::new())
            }
        }
    }
}
