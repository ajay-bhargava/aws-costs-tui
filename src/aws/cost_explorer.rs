//! AWS Cost Explorer API client with SigV4 signing

use anyhow::{anyhow, Result};
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use chrono::{Datelike, Duration, Local, NaiveDate};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use tracing::debug;

use super::Credentials;

/// Cost Explorer API client
pub struct CostExplorerClient {
    credentials: Credentials,
    client: Client,
}

/// Time period for cost queries
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TimePeriod {
    pub start: String,
    pub end: String,
}

/// Group definition for cost queries
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GroupDefinition {
    #[serde(rename = "Type")]
    pub group_type: String,
    pub key: String,
}

/// Cost and usage request
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct GetCostAndUsageRequest {
    time_period: TimePeriod,
    granularity: String,
    metrics: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_by: Option<Vec<GroupDefinition>>,
}

/// Cost and usage response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetCostAndUsageResponse {
    pub results_by_time: Vec<ResultByTime>,
}

/// Results grouped by time period
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
pub struct ResultByTime {
    pub time_period: TimePeriodResponse,
    pub total: Option<std::collections::HashMap<String, MetricValue>>,
    pub groups: Option<Vec<Group>>,
}

/// Time period in response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
pub struct TimePeriodResponse {
    pub start: String,
    pub end: String,
}

/// Group in response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Group {
    pub keys: Vec<String>,
    pub metrics: std::collections::HashMap<String, MetricValue>,
}

/// Metric value
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct MetricValue {
    pub amount: String,
    pub unit: Option<String>,
}

/// Processed cost data for display
#[derive(Debug, Clone)]
pub struct CostData {
    pub period: String,
    pub total_cost: f64,
    pub currency: String,
    pub breakdown: Vec<ServiceCost>,
}

/// Cost breakdown by service
#[derive(Debug, Clone)]
pub struct ServiceCost {
    pub service: String,
    pub cost: f64,
    pub percentage: f64,
}

impl CostExplorerClient {
    /// Create a new Cost Explorer client
    pub fn new(credentials: Credentials) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { credentials, client }
    }

    /// Get the Cost Explorer endpoint URL
    fn endpoint(&self) -> String {
        format!("https://ce.{}.amazonaws.com", self.credentials.region)
    }

    /// Sign and execute a request to Cost Explorer API
    fn execute_request(&self, action: &str, body: &str) -> Result<String> {
        let endpoint = self.endpoint();
        let now = SystemTime::now();

        // Create AWS credentials and convert to Identity
        let aws_creds = aws_credential_types::Credentials::new(
            &self.credentials.access_key_id,
            &self.credentials.secret_access_key,
            self.credentials.session_token.clone(),
            None,
            "aws-costs-tui",
        );
        let identity: Identity = aws_creds.into();

        let signing_settings = SigningSettings::default();
        let signing_params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.credentials.region)
            .name("ce") // Cost Explorer service name
            .time(now)
            .settings(signing_settings)
            .build()
            .map_err(|e| anyhow!("Failed to build signing params: {}", e))?;

        // Format headers for signing
        let x_amz_target = format!("AWSInsightsIndexService.{}", action);
        let host = format!("ce.{}.amazonaws.com", self.credentials.region);

        // Create the signable request
        let signable_request = SignableRequest::new(
            "POST",
            &endpoint,
            [
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", x_amz_target.as_str()),
                ("host", host.as_str()),
            ]
            .into_iter(),
            SignableBody::Bytes(body.as_bytes()),
        )
        .map_err(|e| anyhow!("Failed to create signable request: {}", e))?;

        // Sign the request
        let (signing_instructions, _signature) = sign(signable_request, &signing_params.into())
            .map_err(|e| anyhow!("Failed to sign request: {}", e))?
            .into_parts();

        // Build the actual request with signed headers
        let mut request_builder = self
            .client
            .post(&endpoint)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", &x_amz_target)
            .body(body.to_string());

        // Apply signing headers
        for (name, value) in signing_instructions.headers() {
            let header_name: &str = name.as_ref();
            let header_value: &str = value.as_ref();
            request_builder = request_builder.header(header_name, header_value);
        }

        debug!("Executing Cost Explorer API request: {}", action);

        let response = request_builder
            .send()
            .map_err(|e| anyhow!("Request failed: {}", e))?;

        let status = response.status();
        let response_body = response
            .text()
            .map_err(|e| anyhow!("Failed to read response: {}", e))?;

        if !status.is_success() {
            return Err(anyhow!(
                "API request failed with status {}: {}",
                status,
                response_body
            ));
        }

        Ok(response_body)
    }

    /// Get cost and usage data
    pub fn get_cost_and_usage(
        &self,
        time_period: TimePeriod,
        granularity: &str,
        group_by_service: bool,
    ) -> Result<GetCostAndUsageResponse> {
        let group_by = if group_by_service {
            Some(vec![GroupDefinition {
                group_type: "DIMENSION".to_string(),
                key: "SERVICE".to_string(),
            }])
        } else {
            None
        };

        let request = GetCostAndUsageRequest {
            time_period,
            granularity: granularity.to_string(),
            metrics: vec!["UnblendedCost".to_string()],
            group_by,
        };

        let body = serde_json::to_string(&request)
            .map_err(|e| anyhow!("Failed to serialize request: {}", e))?;

        let response_body = self.execute_request("GetCostAndUsage", &body)?;

        let response: GetCostAndUsageResponse = serde_json::from_str(&response_body)
            .map_err(|e| anyhow!("Failed to parse response: {} - Body: {}", e, response_body))?;

        Ok(response)
    }

    /// Get monthly costs broken down by service for the current month
    pub fn get_current_month_costs(&self) -> Result<CostData> {
        let today = Local::now().date_naive();
        let start_of_month = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
            .ok_or_else(|| anyhow!("Failed to calculate start of month"))?;
        
        // End date is tomorrow (exclusive)
        let end_date = today + Duration::days(1);

        let time_period = TimePeriod {
            start: start_of_month.format("%Y-%m-%d").to_string(),
            end: end_date.format("%Y-%m-%d").to_string(),
        };

        self.get_costs_for_period(time_period, &format!("{}", today.format("%B %Y")))
    }

    /// Get costs for the previous month
    pub fn get_previous_month_costs(&self) -> Result<CostData> {
        let today = Local::now().date_naive();
        let first_of_current = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
            .ok_or_else(|| anyhow!("Failed to calculate first of current month"))?;
        
        let last_of_previous = first_of_current - Duration::days(1);
        let first_of_previous = NaiveDate::from_ymd_opt(
            last_of_previous.year(),
            last_of_previous.month(),
            1,
        )
        .ok_or_else(|| anyhow!("Failed to calculate first of previous month"))?;

        let time_period = TimePeriod {
            start: first_of_previous.format("%Y-%m-%d").to_string(),
            end: first_of_current.format("%Y-%m-%d").to_string(),
        };

        self.get_costs_for_period(time_period, &format!("{}", first_of_previous.format("%B %Y")))
    }

    /// Get last N months of costs
    pub fn get_monthly_trend(&self, months: u32) -> Result<Vec<CostData>> {
        let mut results = Vec::new();
        let today = Local::now().date_naive();

        for i in 0..months {
            let target_month = today - Duration::days(30 * i as i64);
            let first_of_month = NaiveDate::from_ymd_opt(target_month.year(), target_month.month(), 1)
                .ok_or_else(|| anyhow!("Failed to calculate month"))?;
            
            let next_month = if target_month.month() == 12 {
                NaiveDate::from_ymd_opt(target_month.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(target_month.year(), target_month.month() + 1, 1)
            }
            .ok_or_else(|| anyhow!("Failed to calculate next month"))?;

            // For current month, end at tomorrow
            let end_date = if i == 0 {
                std::cmp::min(today + Duration::days(1), next_month)
            } else {
                next_month
            };

            let time_period = TimePeriod {
                start: first_of_month.format("%Y-%m-%d").to_string(),
                end: end_date.format("%Y-%m-%d").to_string(),
            };

            let period_name = first_of_month.format("%B %Y").to_string();
            match self.get_costs_for_period(time_period, &period_name) {
                Ok(data) => results.push(data),
                Err(e) => debug!("Failed to get costs for {}: {}", period_name, e),
            }
        }

        results.reverse(); // Oldest to newest
        Ok(results)
    }

    /// Get costs for a specific period with service breakdown
    fn get_costs_for_period(&self, time_period: TimePeriod, period_name: &str) -> Result<CostData> {
        let response = self.get_cost_and_usage(time_period, "MONTHLY", true)?;

        let mut total_cost = 0.0;
        let mut currency = "USD".to_string();
        let mut service_costs: Vec<ServiceCost> = Vec::new();

        for result in &response.results_by_time {
            if let Some(groups) = &result.groups {
                for group in groups {
                    let service_name = group.keys.first().cloned().unwrap_or_default();
                    if let Some(metric) = group.metrics.get("UnblendedCost") {
                        let cost: f64 = metric.amount.parse().unwrap_or(0.0);
                        if cost > 0.001 {
                            total_cost += cost;
                            if let Some(unit) = &metric.unit {
                                currency = unit.clone();
                            }
                            service_costs.push(ServiceCost {
                                service: service_name,
                                cost,
                                percentage: 0.0, // Will calculate after
                            });
                        }
                    }
                }
            }
        }

        // Calculate percentages and sort by cost descending
        for service in &mut service_costs {
            service.percentage = if total_cost > 0.0 {
                (service.cost / total_cost) * 100.0
            } else {
                0.0
            };
        }
        service_costs.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal));

        Ok(CostData {
            period: period_name.to_string(),
            total_cost,
            currency,
            breakdown: service_costs,
        })
    }
}
