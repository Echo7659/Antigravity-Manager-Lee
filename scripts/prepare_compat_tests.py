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
assert 'fn extract_apply_patch_input' in response
(destination / 'streaming_excerpt.rs').write_text(streaming.split('pub fn create_legacy_sse_stream', 1)[0])
(destination / 'response_excerpt.rs').write_text(response.split('fn extract_apply_patch_input', 1)[0])


common = (source / 'handlers/common.rs').read_text()
common = common.split('/// Detects model capabilities and configuration', 1)[0]
common = common.replace('use crate::proxy::server::AppState;\n', '')
(destination / 'retry_common.rs').write_text(common)


def module(path):
    return json.dumps(str(path))


# 缓存与音频桩仅隔离未参与这些测试的应用依赖，协议转换代码保持原样。
(destination / 'lib.rs').write_text(f'''#![allow(dead_code, unused_imports)]
#[path = {module(source / 'mappers/gemini/request_compat.rs')}]
mod request_compat;
#[path = {module(source / 'handlers/account_attempts.rs')}]
mod account_attempts;
#[path = {module(destination / 'retry_common.rs')}]
mod retry_common;
pub mod proxy {{
    pub mod upstream {{
        #[path = {module(source / 'upstream/retry.rs')}] pub mod retry;
    }}
    pub struct SignatureCache;
    impl SignatureCache {{
        pub fn global() -> &'static Self {{ &Self }}
        pub fn cache_session_signature(&self, _: &str, _: String, _: usize) {{}}
    }}
    pub mod audio {{ pub fn normalize_audio_mime(value: &str) -> String {{ value.to_string() }} }}
    pub mod mappers {{
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
manifest = '''[package]
name = "antigravity-compat-tests"
version = "0.1.0"
edition = "2021"
[lib]
path = "lib.rs"
[dependencies]
'''
features = {'serde': ['derive'], 'serde_json': ['preserve_order'], 'uuid': ['v4'], 'tokio': ['macros', 'rt', 'time']}
for name in ['serde', 'serde_json', 'bytes', 'futures', 'chrono', 'rand', 'uuid', 'tracing', 'async-stream', 'tokio', 'axum', 'once_cell', 'regex']:
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
