mod status;

use lxy::Router;
use lxy::routing::RouterService;

pub(super) fn build() -> RouterService {
  let mut router = Router::new();
  router.get("/health", health);
  router.get("/status", status::get);
  router.build()
}

async fn health() -> &'static str {
  "ok"
}
