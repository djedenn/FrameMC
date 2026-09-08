use crate::error::ProxyError;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Metadata and compiled AST for an individual plugin.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub path: String,
    pub ast: rhai::AST,
}

/// Sandboxed Rhai script host enforcing deterministic execution limits ([R-04]).
pub struct ScriptHost {
    pub(crate) engine: rhai::Engine,
    pub(crate) ast: RwLock<rhai::AST>,
    pub(crate) plugins: RwLock<Vec<PluginInfo>>,
    servers: Arc<std::sync::RwLock<Vec<String>>>,
    default_server: Arc<std::sync::RwLock<String>>,
}

impl Default for ScriptHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptHost {
    /// Creates a new `ScriptHost` configured with strict security limits and thread-safe logging helpers.
    pub fn new() -> Self {
        let mut engine = rhai::Engine::new();

        // [R-04] Deterministic script limits
        engine.set_max_operations(50_000);
        engine.set_max_call_levels(32);
        engine.set_max_string_size(1024);
        engine.set_max_expr_depths(128, 64);

        // Disable external file access from inside scripts
        engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());

        // Thread-safe logging helpers
        engine.register_fn("proxy_info", |msg: &str| {
            tracing::info!("[Rhai] {}", msg);
        });
        engine.register_fn("proxy_warn", |msg: &str| {
            tracing::warn!("[Rhai] {}", msg);
        });
        engine.register_fn("proxy_error", |msg: &str| {
            tracing::error!("[Rhai] {}", msg);
        });

        let servers: Arc<std::sync::RwLock<Vec<String>>> =
            Arc::new(std::sync::RwLock::new(Vec::new()));
        let default_server = Arc::new(std::sync::RwLock::new("lobby".to_string()));

        let servers_clone = Arc::clone(&servers);
        engine.register_fn("get_servers", move || -> rhai::Array {
            let guard = servers_clone.read().unwrap();
            guard
                .iter()
                .map(|s| rhai::Dynamic::from(s.clone()))
                .collect()
        });

        let default_clone = Arc::clone(&default_server);
        engine.register_fn("get_default_server", move || -> String {
            default_clone.read().unwrap().clone()
        });

        let servers_exists = Arc::clone(&servers);
        engine.register_fn("server_exists", move |name: &str| -> bool {
            let guard = servers_exists.read().unwrap();
            guard.iter().any(|s| s.eq_ignore_ascii_case(name))
        });

        // Thread-safe in-memory key-value store for plugin state persistence
        let kv_store: Arc<std::sync::RwLock<std::collections::HashMap<String, rhai::Dynamic>>> =
            Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        let kv_set = Arc::clone(&kv_store);
        engine.register_fn("kv_set", move |key: &str, value: rhai::Dynamic| {
            if let Ok(mut guard) = kv_set.write() {
                guard.insert(key.to_string(), value);
            }
        });

        let kv_get = Arc::clone(&kv_store);
        engine.register_fn("kv_get", move |key: &str| -> rhai::Dynamic {
            if let Ok(guard) = kv_get.read() {
                guard.get(key).cloned().unwrap_or(rhai::Dynamic::UNIT)
            } else {
                rhai::Dynamic::UNIT
            }
        });

        let kv_has = Arc::clone(&kv_store);
        engine.register_fn("kv_has", move |key: &str| -> bool {
            if let Ok(guard) = kv_has.read() {
                guard.contains_key(key)
            } else {
                false
            }
        });

        let kv_remove = Arc::clone(&kv_store);
        engine.register_fn("kv_remove", move |key: &str| -> bool {
            if let Ok(mut guard) = kv_remove.write() {
                guard.remove(key).is_some()
            } else {
                false
            }
        });

        let kv_keys = Arc::clone(&kv_store);
        engine.register_fn("kv_keys", move |prefix: &str| -> rhai::Array {
            if let Ok(guard) = kv_keys.read() {
                let mut matches: Vec<String> = guard
                    .keys()
                    .filter(|k| prefix.is_empty() || k.starts_with(prefix))
                    .cloned()
                    .collect();
                matches.sort();
                matches.into_iter().map(rhai::Dynamic::from).collect()
            } else {
                Vec::new()
            }
        });

        let kv_clear = Arc::clone(&kv_store);
        engine.register_fn("kv_clear", move || {
            if let Ok(mut guard) = kv_clear.write() {
                guard.clear();
            }
        });

        // Time utility functions for timeouts, cooldowns, and durations
        engine.register_fn("timestamp_sec", || -> i64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });

        engine.register_fn("timestamp_ms", || -> i64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        });

        Self {
            engine,
            ast: RwLock::new(rhai::AST::default()),
            plugins: RwLock::new(Vec::new()),
            servers,
            default_server,
        }
    }

    /// Updates the configured servers known to Rhai plugins.
    pub fn set_servers(&self, servers: Vec<String>, default_server: String) {
        if let Ok(mut guard) = self.servers.write() {
            *guard = servers;
        }
        if let Ok(mut guard) = self.default_server.write() {
            *guard = default_server;
        }
    }

    /// Compiles and registers an individual `.rhai` plugin script from a file.
    pub async fn load_plugin_file(&self, path: &str) -> Result<(), ProxyError> {
        let script_content = tokio::fs::read_to_string(path).await.map_err(|e| {
            ProxyError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read plugin file at '{path}': {e}"),
            ))
        })?;

        let name = Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin")
            .to_string();

        let clean_content = script_content
            .strip_prefix('\u{feff}')
            .unwrap_or(&script_content);

        let ast = self.engine.compile(clean_content).map_err(|e| {
            ProxyError::ScriptError(format!("Failed to compile plugin '{path}': {e}"))
        })?;

        let mut plugins = self.plugins.write().await;
        if let Some(existing) = plugins.iter_mut().find(|p| p.name == name) {
            existing.path = path.to_string();
            existing.ast = ast;
        } else {
            plugins.push(PluginInfo {
                name: name.clone(),
                path: path.to_string(),
                ast,
            });
        }

        tracing::info!("Successfully loaded plugin '{}' from '{}'", name, path);
        Ok(())
    }

    /// Scans the given directory and loads all `*.rhai` plugin files.
    pub async fn load_plugins_dir(&self, dir_path: &str) -> Result<usize, ProxyError> {
        let p = Path::new(dir_path);
        if !p.exists() {
            if let Err(e) = tokio::fs::create_dir_all(p).await {
                tracing::warn!("Failed to create plugins directory '{}': {e}", dir_path);
                return Ok(0);
            }
        }

        let mut entries = match tokio::fs::read_dir(dir_path).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Failed to read plugins directory '{}': {e}", dir_path);
                return Ok(0);
            }
        };

        let mut files = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("rhai") {
                files.push(path.to_string_lossy().to_string());
            }
        }

        files.sort();
        let mut loaded = 0;
        for file in files {
            match self.load_plugin_file(&file).await {
                Ok(()) => loaded += 1,
                Err(e) => tracing::error!("Error loading plugin '{file}': {e}"),
            }
        }

        tracing::info!("Loaded {} plugin(s) from '{}'", loaded, dir_path);
        Ok(loaded)
    }

    /// Compiles and loads a script from a raw string into the active AST.
    pub async fn load_script(&self, script: &str) -> Result<(), ProxyError> {
        let new_ast = self
            .engine
            .compile(script)
            .map_err(|e| ProxyError::ScriptError(format!("Failed to compile script: {e}")))?;

        let mut ast_guard = self.ast.write().await;
        *ast_guard = new_ast;
        Ok(())
    }

    /// Compiles a script file from disk and atomically swaps the active AST.
    pub async fn reload(&self, path: &str) -> Result<(), ProxyError> {
        let script_content = tokio::fs::read_to_string(path).await.map_err(|e| {
            ProxyError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read script file at '{path}': {e}"),
            ))
        })?;

        let clean_content = script_content
            .strip_prefix('\u{feff}')
            .unwrap_or(&script_content);

        let new_ast = self.engine.compile(clean_content).map_err(|e| {
            ProxyError::ScriptError(format!("Failed to compile script '{path}': {e}"))
        })?;

        let mut ast_guard = self.ast.write().await;
        *ast_guard = new_ast;
        tracing::info!("Successfully loaded and compiled Rhai script from '{path}'");
        Ok(())
    }

    /// Returns a reference to the underlying Rhai `Engine`.
    pub fn engine(&self) -> &rhai::Engine {
        &self.engine
    }

    /// Evaluates top-level statements in the currently active AST.
    pub async fn eval_active<T: Clone + Send + Sync + 'static>(&self) -> Result<T, ProxyError> {
        let ast = self.ast.read().await;
        self.engine
            .eval_ast::<T>(&ast)
            .map_err(|e| ProxyError::ScriptError(format!("Rhai execution error: {e}")))
    }

    /// Calls a function defined in the active AST or any loaded plugin with the given arguments.
    pub async fn call_fn<T: Clone + Send + Sync + 'static>(
        &self,
        fn_name: &str,
        args: impl rhai::FuncArgs,
    ) -> Result<T, ProxyError> {
        let plugins = self.plugins.read().await;
        for plugin in plugins.iter() {
            if plugin.ast.iter_functions().any(|f| f.name == fn_name) {
                let mut scope = rhai::Scope::new();
                return self
                    .engine
                    .call_fn::<T>(&mut scope, &plugin.ast, fn_name, args)
                    .map_err(|e| {
                        ProxyError::ScriptError(format!(
                            "Rhai call to '{fn_name}' in plugin '{}' failed: {e}",
                            plugin.name
                        ))
                    });
            }
        }

        let ast = self.ast.read().await;
        let mut scope = rhai::Scope::new();
        self.engine
            .call_fn(&mut scope, &ast, fn_name, args)
            .map_err(|e| ProxyError::ScriptError(format!("Rhai call to '{fn_name}' failed: {e}")))
    }

    /// Checks whether a given function is defined in the currently loaded AST or any plugin.
    pub async fn has_function(&self, fn_name: &str) -> bool {
        let plugins = self.plugins.read().await;
        if plugins
            .iter()
            .any(|p| p.ast.iter_functions().any(|f| f.name == fn_name))
        {
            return true;
        }
        let ast = self.ast.read().await;
        let exists = ast.iter_functions().any(|f| f.name == fn_name);
        exists
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[tokio::test]
    async fn test_infinite_loop_halts_with_operations_limit() {
        let host = ScriptHost::new();
        host.load_script("while true {}").await.unwrap();

        let res: Result<(), ProxyError> = host.eval_active().await;
        assert!(
            res.is_err(),
            "Infinite loop must fail with operations limit error"
        );

        match res {
            Err(ProxyError::ScriptError(msg)) => {
                let lower = msg.to_lowercase();
                assert!(
                    lower.contains("too many operations") || lower.contains("operations limit"),
                    "Expected operations limit exceeded error, got: {msg}"
                );
            }
            other => panic!("Expected ScriptError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_recursion_depth_limit() {
        let host = ScriptHost::new();
        let script = r#"
            fn recurse(n) {
                recurse(n + 1)
            }
            recurse(0)
        "#;
        host.load_script(script).await.unwrap();

        let res: Result<(), ProxyError> = host.eval_active().await;
        assert!(res.is_err(), "Deep recursion must fail call levels limit");

        match res {
            Err(ProxyError::ScriptError(msg)) => {
                let lower = msg.to_lowercase();
                assert!(
                    lower.contains("call") || lower.contains("depth") || lower.contains("stack"),
                    "Expected call stack / level limit error, got: {msg}"
                );
            }
            other => panic!("Expected ScriptError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_max_string_size_limit() {
        let host = ScriptHost::new();
        // Trying to construct a string exceeding 1024 characters
        let script = r#"
            let s = "a";
            while s.len < 2000 {
                s += s;
            }
            s
        "#;
        host.load_script(script).await.unwrap();

        let res: Result<String, ProxyError> = host.eval_active().await;
        assert!(res.is_err(), "String exceeding 1024 chars must be rejected");

        match res {
            Err(ProxyError::ScriptError(msg)) => {
                let lower = msg.to_lowercase();
                assert!(
                    lower.contains("string") || lower.contains("size") || lower.contains("limit"),
                    "Expected string size limit error, got: {msg}"
                );
            }
            other => panic!("Expected ScriptError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_external_file_import_is_disabled() {
        let host = ScriptHost::new();
        let script = r#"
            import "some_external_file" as ext;
        "#;
        let res = host.load_script(script).await;
        // Compiling or running import with DummyModuleResolver should fail
        if res.is_ok() {
            let eval_res: Result<(), ProxyError> = host.eval_active().await;
            assert!(
                eval_res.is_err(),
                "Module import must fail with DummyModuleResolver"
            );
        }
    }

    #[tokio::test]
    async fn test_logging_functions_and_call_fn() {
        let host = ScriptHost::new();
        let script = r#"
            proxy_info("Hello from proxy_info test");
            proxy_warn("Hello from proxy_warn test");

            fn on_player_join(event) {
                if event.username == "Notch" {
                    #{ allow: true, target_server: "lobby" }
                } else {
                    #{ allow: false, disconnect_reason: "Whitelisted only" }
                }
            }
        "#;
        host.load_script(script).await.unwrap();

        // 1. Eval top-level statements (including proxy_info and proxy_warn)
        let eval_res: Result<(), ProxyError> = host.eval_active().await;
        assert!(
            eval_res.is_ok(),
            "Top-level evaluation and logging must succeed"
        );

        // 2. Call on_player_join with Map event
        let mut event = rhai::Map::new();
        event.insert("username".into(), rhai::Dynamic::from("Notch".to_string()));

        let result: rhai::Map = host.call_fn("on_player_join", (event,)).await.unwrap();
        assert!(result.get("allow").unwrap().as_bool().unwrap());
        assert_eq!(
            result
                .get("target_server")
                .unwrap()
                .clone()
                .into_string()
                .unwrap(),
            "lobby"
        );

        // Test non-whitelisted
        let mut event2 = rhai::Map::new();
        event2.insert(
            "username".into(),
            rhai::Dynamic::from("BannedPlayer".to_string()),
        );

        let result2: rhai::Map = host.call_fn("on_player_join", (event2,)).await.unwrap();
        assert!(!result2.get("allow").unwrap().as_bool().unwrap());
        assert_eq!(
            result2
                .get("disconnect_reason")
                .unwrap()
                .clone()
                .into_string()
                .unwrap(),
            "Whitelisted only"
        );
    }

    #[tokio::test]
    async fn test_reload_from_file() {
        let temp_dir = std::env::temp_dir();
        let script_file = temp_dir.join(format!("framemc_test_{}.rhai", rand::random::<u32>()));

        let initial_script = r#"
            fn get_val() { 10 }
        "#;
        tokio::fs::write(&script_file, initial_script)
            .await
            .unwrap();

        let host = ScriptHost::new();
        host.reload(script_file.to_str().unwrap()).await.unwrap();

        let val1: i64 = host.call_fn("get_val", ()).await.unwrap();
        assert_eq!(val1, 10);

        // Update file and reload
        let updated_script = r#"
            fn get_val() { 42 }
        "#;
        tokio::fs::write(&script_file, updated_script)
            .await
            .unwrap();
        host.reload(script_file.to_str().unwrap()).await.unwrap();

        let val2: i64 = host.call_fn("get_val", ()).await.unwrap();
        assert_eq!(val2, 42);

        // Clean up
        let _ = tokio::fs::remove_file(&script_file).await;
    }

    #[tokio::test]
    async fn test_plugin_directory_loading_and_server_queries() {
        let temp_dir =
            std::env::temp_dir().join(format!("framemc_plugins_{}", rand::random::<u32>()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let plugin1 = r#"
            fn get_plugin_one() { "one" }
        "#;
        tokio::fs::write(temp_dir.join("01_test.rhai"), plugin1)
            .await
            .unwrap();

        let plugin2 = r#"
            fn check_servers() {
                let s = get_servers();
                let def = get_default_server();
                let has_steel = server_exists("steelmc");
                let has_none = server_exists("nonexistent");
                #{ server_count: s.len, def_server: def, has_steel: has_steel, has_none: has_none }
            }
        "#;
        tokio::fs::write(temp_dir.join("02_server.rhai"), plugin2)
            .await
            .unwrap();

        let host = ScriptHost::new();
        host.set_servers(
            vec!["lobby".to_string(), "steelmc".to_string()],
            "lobby".to_string(),
        );

        let loaded = host
            .load_plugins_dir(temp_dir.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(loaded, 2);

        let val1: String = host.call_fn("get_plugin_one", ()).await.unwrap();
        assert_eq!(val1, "one");

        let res_map: rhai::Map = host.call_fn("check_servers", ()).await.unwrap();
        assert_eq!(res_map.get("server_count").unwrap().as_int().unwrap(), 2);
        assert_eq!(
            res_map
                .get("def_server")
                .unwrap()
                .clone()
                .into_string()
                .unwrap(),
            "lobby"
        );
        assert!(res_map.get("has_steel").unwrap().as_bool().unwrap());
        assert!(!res_map.get("has_none").unwrap().as_bool().unwrap());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_kv_store_and_timestamps() {
        let host = ScriptHost::new();
        let script = r#"
            fn test_kv() {
                kv_set("ban:Alex", #{ reason: "Cheat", expires_at: 0 });
                kv_set("online:Alex", "lobby");
                kv_set("online:Steve", "steelmc");

                let has_alex_ban = kv_has("ban:Alex");
                let ban_info = kv_get("ban:Alex");
                let reason = ban_info.reason;

                let online = kv_keys("online:");
                let sec = timestamp_sec();
                let ms = timestamp_ms();

                kv_remove("online:Alex");
                let online_after = kv_keys("online:");

                let words = "hello world from rhai".split(" ");
                let idx = "test/command".index_of("/");

                #{
                    has_ban: has_alex_ban,
                    reason: reason,
                    online_count: online.len,
                    online_after_count: online_after.len,
                    valid_time: (sec > 0) && (ms >= sec * 1000) && (words.len == 4) && (idx == 4)
                }
            }
        "#;
        host.load_script(script).await.unwrap();

        let res: rhai::Map = host.call_fn("test_kv", ()).await.unwrap();
        assert!(res.get("has_ban").unwrap().as_bool().unwrap());
        assert_eq!(
            res.get("reason").unwrap().clone().into_string().unwrap(),
            "Cheat"
        );
        assert_eq!(res.get("online_count").unwrap().as_int().unwrap(), 2);
        assert_eq!(res.get("online_after_count").unwrap().as_int().unwrap(), 1);
        assert!(res.get("valid_time").unwrap().as_bool().unwrap());
    }
}
