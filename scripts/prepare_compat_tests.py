"""生成隔离回归测试工程，测试代码直接引用当前仓库的转换器实现。"""
import argparse
import json
import pathlib
import shutil
import tomllib

parser = argparse.ArgumentParser()
parser.add_argument('destination', type=pathlib.Path)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[1]
source = root / 'src-tauri/src/proxy'
destination = args.destination.resolve()
destination.mkdir(parents=True, exist_ok=True)
packages = tomllib.loads((root / 'src-tauri/Cargo.lock').read_text())['package']


def version(name, prefix=''):
    return next(package['version'] for package in packages if package['name'] == name and package['version'].startswith(prefix))


streaming = (source / 'mappers/openai/streaming.rs').read_text()
response = (source / 'mappers/openai/response.rs').read_text()
assert 'pub fn create_legacy_sse_stream' in streaming
assert '#[cfg(test)]' in response
(destination / 'streaming_excerpt.rs').write_text(streaming.split('pub fn create_legacy_sse_stream', 1)[0])
(destination / 'response_excerpt.rs').write_text(response.split('#[cfg(test)]', 1)[0])


common = (source / 'handlers/common.rs').read_text()
common_prefix = common.split('/// Detects model capabilities and configuration', 1)[0]
start = common.index('pub fn is_model_not_found_error(')
end = common.index('\n}', start) + 2
common = common_prefix + '\n' + common[start:end]

common = common.replace('use crate::proxy::server::AppState;\n', '')
(destination / 'retry_common.rs').write_text(common)


utils = (source / 'mappers/common_utils.rs').read_text()
start = utils.index('pub fn safe_truncate_chars(')
end = utils.index('\n}', start) + 2
(destination / 'common_utils_excerpt.rs').write_text(utils[start:end])


def module(path):
    return json.dumps(str(path))


# 缓存与音频桩仅隔离未参与这些测试的应用依赖，协议转换代码保持原样。
(destination / 'lib.rs').write_text(f'''#![allow(dead_code, unused_imports)]
#[path = {module(source / 'mappers/gemini/request_compat.rs')}]
mod request_compat;
#[path = {module(source / 'handlers/account_attempts.rs')}]
mod account_attempts;
#[path = {module(source / "account_ranking.rs")}]
mod account_ranking;
#[path = {module(source / "quota_policy.rs")}]
mod quota_policy;
pub mod models {{
    #[path = {module(root / "src-tauri/src/models/quota.rs")}] pub mod quota;
}}
#[path = {module(destination / 'retry_common.rs')}]
mod retry_common;
pub mod proxy {{
    pub mod pipeline {{
        #[path = {module(source / 'pipeline/usage.rs')}] pub mod usage;
        #[path = {module(source / 'pipeline/policy.rs')}] pub mod policy;
        pub use usage::CanonicalUsage;
        pub use policy::UpstreamClassification;
    }}
    pub mod adapters {{
        #[path = {module(source / 'adapters/apply_patch_preflight.rs')}] pub mod apply_patch_preflight;
    }}
    pub mod thinking_store {{
        pub struct TurnAccumulator;
        impl TurnAccumulator {{
            pub fn new() -> Self {{ Self }}
            pub fn with_anchor(_: &str) -> Self {{ Self }}
            pub fn ingest_part(&mut self, _: &serde_json::Value) {{}}
            pub fn record_tool_id(&mut self, _: &str, _: &str) {{}}
            pub fn commit(self, _: &str) {{}}
        }}
        pub fn capture_gemini_parts(_: &str, _: &[serde_json::Value]) {{}}
        pub fn capture_gemini_parts_with_anchor(_: &str, _: &[serde_json::Value], _: &str) {{}}
    }}
    pub mod token_manager {{
        #[derive(Default)]
        pub struct TokenManager {{ pub refreshes: std::sync::atomic::AtomicUsize }}
        impl TokenManager {{
            pub fn schedule_quota_refresh(self: &std::sync::Arc<Self>, _: &str) {{
                self.refreshes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }}
        }}
    }}
    pub mod server {{
        #[derive(Clone)]
        pub struct AppState {{ pub token_manager: std::sync::Arc<super::token_manager::TokenManager> }}
    }}
    #[path = {module(source / 'middleware/quota_refresh.rs')}] pub mod quota_refresh;
    #[path = {module(source / 'middleware/response_deadline.rs')}] pub mod response_deadline;

    pub mod upstream {{
        #[path = {module(source / 'upstream/retry.rs')}] pub mod retry;
        #[path = {module(source / 'upstream/header_timeout.rs')}] pub mod header_timeout;
    }}
    pub struct SignatureCache;
    impl SignatureCache {{
        pub fn global() -> &'static Self {{ &Self }}
        pub fn cache_tool_signature(&self, _: &str, _: String) {{}}
        pub fn cache_session_signature(&self, _: &str, _: String, _: usize) {{}}
    }}
    pub mod audio {{ pub fn normalize_audio_mime(value: &str) -> String {{ value.to_string() }} }}
    pub mod mappers {{
        #[path = {module(destination / "common_utils_excerpt.rs")}] pub mod common_utils;
        #[path = {module(source / 'mappers/usage.rs')}] pub mod usage;
        #[path = {module(source / 'mappers/error_classifier.rs')}] pub mod error_classifier;
        pub mod gemini {{
            #[path = {module(source / 'mappers/gemini/collector.rs')}] pub mod collector;
        }}
        pub mod openai {{
            #[path = {module(source / 'mappers/openai/models.rs')}] pub mod models;
            #[path = {module(destination / 'response_excerpt.rs')}] pub mod response;
            #[path = {module(destination / 'streaming_excerpt.rs')}] pub mod streaming;
            #[cfg(test)]
            #[path = {module(source / 'mappers/openai/streaming_compat_tests.rs')}] mod streaming_compat_tests;
        }}
    }}
}}
''')
with (destination / 'lib.rs').open('a') as tests:
    tests.write('''
#[cfg(test)]
mod quota_refresh_tests {
    use super::proxy::{quota_refresh::wrap_quota_refresh, token_manager::TokenManager};
    use axum::body::Body;
    use std::sync::{Arc, atomic::Ordering};
    use futures::StreamExt;
    #[tokio::test]
    async fn compat_quota_refresh_runs_after_response_body_finishes() {
        let manager = Arc::new(TokenManager::default());
        let response = axum::response::Response::builder().header("x-account-email", "test@example.com").body(Body::from("OK")).unwrap();
        let response = wrap_quota_refresh(response, manager.clone());
        assert_eq!(manager.refreshes.load(Ordering::SeqCst), 0);
        assert_eq!(axum::body::to_bytes(response.into_body(), 100).await.unwrap().as_ref(), b"OK");
        assert_eq!(manager.refreshes.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn compat_quota_refresh_runs_on_client_disconnect() {
        let manager = Arc::new(TokenManager::default());
        let chunks = futures::stream::once(async { Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"chunk")) }).chain(futures::stream::pending());
        let response = axum::response::Response::builder().header("x-account-email", "test@example.com").body(Body::from_stream(chunks)).unwrap();
        let mut body = wrap_quota_refresh(response, manager.clone()).into_body().into_data_stream();
        assert!(body.next().await.unwrap().is_ok());
        assert_eq!(manager.refreshes.load(Ordering::SeqCst), 0);
        drop(body);
        assert_eq!(manager.refreshes.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn compat_quota_refresh_ignores_health_responses() {
        let manager = Arc::new(TokenManager::default());
        drop(wrap_quota_refresh(axum::response::Response::new(Body::empty()), manager.clone()));
        assert_eq!(manager.refreshes.load(Ordering::SeqCst), 0);
    }
}
''')
manifest = '''[package]
name = "antigravity-compat-tests"
version = "0.1.0"
edition = "2021"
[lib]
path = "lib.rs"
[dependencies]
'''
features = {'serde': ['derive'], 'serde_json': ['preserve_order'], 'uuid': ['v4'], 'tokio': ['macros', 'rt', 'time']}
for name in ['serde', 'serde_json', 'bytes', 'futures', 'chrono', 'rand', 'uuid', 'tracing', 'async-stream', 'tokio', 'axum', 'once_cell', 'regex', 'tempfile']:
    prefix = '0.8.' if name == 'rand' else '1.' if name in ['uuid', 'bytes'] else ''
    dependency = '=' + version(name, prefix)
    if name in features:
        manifest += f'{name} = {{ version = {json.dumps(dependency)}, features = {json.dumps(features[name])} }}\n'
    else:
        manifest += f'{name} = {json.dumps(dependency)}\n'
(destination / 'Cargo.toml').write_text(manifest)
lock = root / 'tests/compat/Cargo.lock'
if lock.exists():
    shutil.copy2(lock, destination / 'Cargo.lock')
print(destination)
