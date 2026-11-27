use super::*;
use std::sync::atomic::AtomicBool;

#[derive(Clone)]
pub(super) struct Policy {
    pub tools: Arc<AtomicBool>,
    pub shell: Arc<AtomicBool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn configured_clients_share_live_policy_without_sharing_each_others_permissions() {
        let mut config = Config::default();
        config.clients.insert(
            "desktop".into(),
            ClientConfig { tools: Some(true), ..ClientConfig::default() },
        );
        let registry = Registry::default();
        let desktop = registry.get("desktop", &config).unwrap();
        let other = registry.get("mobile", &config).unwrap();

        assert!(desktop.tools.load(Ordering::Acquire));
        assert!(!other.tools.load(Ordering::Acquire));

        desktop.tools.store(false, Ordering::Release);

        assert!(!registry.get("desktop", &config).unwrap().tools.load(Ordering::Acquire));
        assert!(!desktop.shell.load(Ordering::Acquire));
        assert!(registry.get("../desktop", &config).is_err());
    }
}

#[derive(Default)]
pub(super) struct Registry {
    items: Mutex<BTreeMap<String, Policy>>,
}

impl Registry {
    pub fn get(&self, id: &str, config: &Config) -> std::io::Result<Policy> {
        if !valid(id) {
            return Err(std::io::Error::other("Invalid client identity."));
        }

        let mut items = self
            .items
            .lock()
            .map_err(|_| std::io::Error::other("Client policy lock is poisoned."))?;

        Ok(items
            .entry(id.into())
            .or_insert_with(|| Policy {
                tools: Arc::new(AtomicBool::new(config.client_tools(id))),
                shell: Arc::new(AtomicBool::new(config.client_shell(id))),
            })
            .clone())
    }
}
