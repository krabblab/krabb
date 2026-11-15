#![forbid(unsafe_code)]

use krabb_core::{
    plugin::serve_events,
    types::{Capability, Hello, Protocol},
};

fn hello() -> Hello {
    Hello {
        formats: vec![],
        protocol: Protocol::CURRENT,
        id: "runner".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        capabilities: vec![Capability::Runner],
        commands: vec![],
    }
}

pub async fn serve() -> krabb_core::Result<()> {
    let backend = krabb_plugin_a2a::Backend::from_env("KRABB_RUNNER")?;

    serve_events(hello(), move |request, emitter| {
        let backend = backend.clone();
        async move { backend.call(request, emitter).await }
    })
    .await
}
