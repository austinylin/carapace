/// HTTP request dispatcher
/// Implemented in Phase 3

use carapace_protocol::{HttpRequest, HttpResponse};

pub struct HttpDispatcher;

impl HttpDispatcher {
    pub fn new() -> Self {
        HttpDispatcher
    }

    /// Dispatch HTTP request to upstream server
    pub async fn dispatch_http(
        &self,
        _req: HttpRequest,
    ) -> anyhow::Result<HttpResponse> {
        Err(anyhow::anyhow!("HTTP dispatch not yet implemented"))
    }
}

impl Default for HttpDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_dispatcher_creation() {
        let _dispatcher = HttpDispatcher::new();
    }
}
