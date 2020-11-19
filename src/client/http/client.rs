use crate::client::http::transport::HttpTransportClient;
use crate::types::client::Error;
use crate::types::jsonrpc::{self, JsonValue};
use std::collections::HashSet;
use std::convert::TryInto;
use std::sync::atomic::{AtomicU64, Ordering};

/// Default maximum request body size (10 MB).
const DEFAULT_MAX_BODY_SIZE_TEN_MB: u32 = 10 * 1024 * 1024;

/// HTTP configuration.
#[derive(Copy, Clone)]
pub struct HttpConfig {
	/// Maximum request body size in bytes.
	pub max_request_body_size: u32,
}

/// JSON-RPC HTTP Client that provides functionality to perform method calls and notifications.
///
/// WARNING: The async methods must be executed on [Tokio 0.2](https://docs.rs/tokio/0.2.22/tokio).
pub struct HttpClient {
	/// HTTP transport client.
	transport: HttpTransportClient,
	/// Request ID that wraps around when overflowing.
	request_id: AtomicU64,
}

impl Default for HttpConfig {
	fn default() -> Self {
		Self { max_request_body_size: DEFAULT_MAX_BODY_SIZE_TEN_MB }
	}
}

impl HttpClient {
	/// Initializes a new HTTP client.
	///
	/// Fails when the URL is invalid.
	pub fn new(target: impl AsRef<str>, config: HttpConfig) -> Result<Self, Error> {
		let transport = HttpTransportClient::new(target, config.max_request_body_size)
			.map_err(|e| Error::TransportError(Box::new(e)))?;
		Ok(Self { transport, request_id: AtomicU64::new(0) })
	}

	/// Send a notification to the server.
	///
	/// WARNING: This method must be executed on [Tokio 0.2](https://docs.rs/tokio/0.2.22/tokio).
	pub async fn notification(
		&self,
		method: impl Into<String>,
		params: impl Into<jsonrpc::Params>,
	) -> Result<(), Error> {
		let request = jsonrpc::Request::Single(jsonrpc::Call::Notification(jsonrpc::Notification {
			jsonrpc: jsonrpc::Version::V2,
			method: method.into(),
			params: params.into(),
		}));

		self.transport.send_notification(request).await.map_err(|e| Error::TransportError(Box::new(e)))
	}

	/// Perform a request towards the server.
	///
	/// WARNING: This method must be executed on [Tokio 0.2](https://docs.rs/tokio/0.2.22/tokio).
	pub async fn request(
		&self,
		method: impl Into<String>,
		params: impl Into<jsonrpc::Params>,
	) -> Result<JsonValue, Error> {
		// NOTE: `fetch_add` wraps on overflow which is intended.
		let id = self.request_id.fetch_add(1, Ordering::SeqCst);
		let request = jsonrpc::Request::Single(jsonrpc::Call::MethodCall(jsonrpc::MethodCall {
			jsonrpc: jsonrpc::Version::V2,
			method: method.into(),
			params: params.into(),
			id: jsonrpc::Id::Num(id),
		}));

		let response = self
			.transport
			.send_request_and_wait_for_response(request)
			.await
			.map_err(|e| Error::TransportError(Box::new(e)))?;

		match response {
			jsonrpc::Response::Single(rp) => Self::process_response(rp, id),
			// Server should not send batch response to a single request.
			jsonrpc::Response::Batch(_rps) => {
				Err(Error::Custom("Server replied with batch response to a single request".to_string()))
			}
			// Server should not reply to a Notification.
			jsonrpc::Response::Notif(_notif) => {
				Err(Error::Custom(format!("Server replied with notification response to request ID: {}", id)))
			}
		}
	}

	/// Perform a batch request towards the server.
	///
	/// Returns `Ok` if all requests were answered successfully.
	/// Returns `Error` if any of the requests fails.
	//
	// TODO(niklasad1): maybe simplify generic `requests`, it's quite unreadable.
	pub async fn batch_request<'a>(
		&self,
		requests: impl IntoIterator<Item = (impl Into<String>, impl Into<jsonrpc::Params>)>,
	) -> Result<Vec<JsonValue>, Error> {
		let mut calls = Vec::new();
		// NOTE(niklasad1): If more than `u64::MAX` requests are performed in the `batch` then duplicate IDs are used
		// which we don't support because ID is used to uniquely identify a given request.
		let mut ids = HashSet::new();

		for (method, params) in requests.into_iter() {
			let id = self.request_id.fetch_add(1, Ordering::SeqCst);
			calls.push(jsonrpc::Call::MethodCall(jsonrpc::MethodCall {
				jsonrpc: jsonrpc::Version::V2,
				method: method.into(),
				params: params.into(),
				id: jsonrpc::Id::Num(id),
			}));
			ids.insert(id);
		}

		let batch_request = jsonrpc::Request::Batch(calls);
		let response = self
			.transport
			.send_request_and_wait_for_response(batch_request)
			.await
			.map_err(|e| Error::TransportError(Box::new(e)))?;

		match response {
			jsonrpc::Response::Single(_) => {
				Err(Error::Custom("Server replied with single response to a batch request".to_string()))
			}
			jsonrpc::Response::Notif(_notif) => {
				Err(Error::Custom("Server replied with notification to a a batch request".to_string()))
			}
			jsonrpc::Response::Batch(rps) => {
				let mut responses = Vec::with_capacity(ids.len());
				for rp in rps {
					let id = match rp.id().as_number() {
						Some(n) => *n,
						_ => return Err(Error::InvalidRequestId),
					};
					if !ids.remove(&id) {
						return Err(Error::InvalidRequestId);
					}
					let val: JsonValue = rp.try_into().map_err(Error::Request)?;
					responses.push(val);
				}
				Ok(responses)
			}
		}
	}

	fn process_response(response: jsonrpc::Output, expected_id: u64) -> Result<JsonValue, Error> {
		match response.id().as_number() {
			Some(n) if n == &expected_id => response.try_into().map_err(Error::Request),
			_ => Err(Error::InvalidRequestId),
		}
	}
}
