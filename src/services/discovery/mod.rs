use std::{fmt::Debug, net::ToSocketAddrs, sync::Arc, time::Duration};

use async_trait::async_trait;

use pingora::{
    server::{ListenFds, ShutdownWatch},
    services::Service,
};
use pingora_load_balancing::{health_check::TcpHealthCheck, selection::RoundRobin, LoadBalancer};
use tokio::sync::broadcast::Sender;
use tracing::debug;

use crate::{
    config::{Config, RouteMatcher},
    stores::routes::RouteStoreContainer,
    MsgProxy, ROUTE_STORE,
};

// Service discovery for load balancers
pub struct RoutingService {
    config: Arc<Config>,
    broadcast: Sender<MsgProxy>,
}

impl RoutingService {
    pub fn new(config: Arc<Config>, broadcast: Sender<MsgProxy>) -> Self {
        Self { config, broadcast }
    }

    /// From a given configuration file, create the static load balancing configuration
    fn add_routes_from_config(&mut self) {
        for route in &self.config.routes {
            // For each upstream, create a backend
            let upstream_backends = route
                .upstreams
                .iter()
                .map(|upstr| format!("{}:{}", upstr.ip, upstr.port))
                .collect::<Vec<String>>();

            add_route_to_router(&route.host, &upstream_backends, route.match_with.clone());

            debug!("Added route: {}, {:?}", route.host, route.upstreams);
        }
    }

    /// Watch for new routes being added and update the Router Store
    fn watch_for_route_changes(&self) -> tokio::task::JoinHandle<()> {
        let mut receiver = self.broadcast.subscribe();

        tokio::spawn(async move {
            loop {
                if let Ok(MsgProxy::NewRoute(route)) = receiver.recv().await {
                    add_route_to_router(&route.host, &route.upstreams, None);
                }
            }
        })
    }
}

#[async_trait]
impl Service for RoutingService {
    async fn start_service(&mut self, _fds: Option<ListenFds>, _shutdown: ShutdownWatch) {
        // Setup initial routes from config file
        self.add_routes_from_config();

        // Watch for new hosts being added and configure them accordingly
        tokio::select! {
            _ = self.watch_for_route_changes() => {}
        };
    }

    fn name(&self) -> &str {
        "proxy_service_discovery"
    }

    fn threads(&self) -> Option<usize> {
        Some(1)
    }
}

// TODO: find if host already exists but new/old upstreams have changed
fn add_route_to_router<A, T>(host: &str, upstream_input: T, match_with: Option<RouteMatcher>)
where
    T: IntoIterator<Item = A> + Debug + Clone + Copy,
    A: ToSocketAddrs,
{
    let upstreams = LoadBalancer::<RoundRobin>::try_from_iter(upstream_input);
    if upstreams.is_err() {
        debug!(
            "Could not create upstreams for host: {}, upstreams {:?}",
            host, upstream_input
        );
        return;
    }

    let mut upstreams = upstreams.unwrap();

    // TODO: support defining health checks in the configuration file
    let tcp_health_check = TcpHealthCheck::new();
    upstreams.set_health_check(tcp_health_check);
    upstreams.health_check_frequency = Some(Duration::from_secs(15));

    // Create new routing container
    let mut route_store_container = RouteStoreContainer::new(Arc::new(upstreams));

    // Prepare route matchers
    // TODO: enable matchers for upstreams for true load balancing based on path
    if let Some(match_with) = match_with {
        // Path matchers
        match match_with.path {
            Some(path_matcher) if path_matcher.patterns.len() > 0 => {
                let pattern = path_matcher.patterns;
                route_store_container.path_matcher.with_pattern(pattern);
            }
            Some(_) => {}
            None => {}
        }
    }

    ROUTE_STORE.insert(host.to_string(), Arc::new(route_store_container));
}
