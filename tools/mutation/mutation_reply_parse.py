"""A reply's cost once parsed is bounded, not only its bytes (`rp`).

The shared estimate in `ai-http/src/json.rs` charges every allocation of a parsed reply against a
budget; each adapter gives its readers a budget and, for OpenAI, a byte cap per kind of reply.
Each case loosens one charge or one budget and runs the test that must notice.
"""

JSON = 'crates/ai-http/src/json.rs'
HTTP = 'craft-ai-http'
O_HTTP = 'crates/ai-openai/src/http.rs'
O_LIB = 'crates/ai-openai/src/lib.rs'
O_PROBE = 'crates/ai-openai/src/probe.rs'
O_JOBS = 'crates/ai-openai/src/jobs.rs'
O_CHAT = 'crates/ai-openai/src/chat.rs'
O_STREAM = 'crates/ai-openai/src/stream.rs'
O = 'craft-ai-openai'
COMFY = 'crates/ai-comfyui/src/http.rs'
UNLIMITED = 'usize::MAX'

RP = [
    # The estimate itself: each part of a parsed tree is charged.
    ('string-not-charged', JSON, 'fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {\n        self.0.charge(v.len())?;', 'fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {', HTTP, 'lib', 'json::tests::each_part_is_charged'),
    ('element-not-charged', JSON, 'self.0.charge(ELEMENT_COST)?;', '', HTTP, 'lib', 'json::tests::each_part_is_charged'),
    ('object-not-charged', JSON, 'self.0.charge(OBJECT_COST)?;', '', HTTP, 'lib', 'json::tests::each_part_is_charged'),
    ('object-cost-understated', JSON, 'pub const OBJECT_COST: usize = 640;', 'pub const OBJECT_COST: usize = 64;', HTTP, 'lib', 'json::tests::a_tiny_object_costs_at_least_a_tree_node'),
    ('entry-not-charged', JSON, 'self.0.charge(ENTRY_COST.saturating_add(key.len()))?;', '', HTTP, 'lib', 'json::tests::each_part_is_charged'),
    ('key-length-not-charged', JSON, 'self.0.charge(ENTRY_COST.saturating_add(key.len()))?;', 'self.0.charge(ENTRY_COST)?;', HTTP, 'lib', 'json::tests::the_budget_is_exact'),
    ('top-value-not-charged', JSON, '.charge::<serde_json::Error>(VALUE_COST)', '.charge::<serde_json::Error>(0)', HTTP, 'lib', 'json::tests::each_part_is_charged'),
    ('over-budget-reported-as-not-json', JSON, 'self.over.set(true);', 'self.over.set(false);', HTTP, 'lib', 'json::tests::many_tiny_objects_are_refused'),
    # OpenAI: a byte cap and a parse budget per kind of reply.
    ('openai-models-take-the-images-budget', O_LIB, 'get_json("/models", http::MODELS_REPLY)', 'get_json("/models", http::IMAGES_REPLY)', O, 'reply_size', 'a_model_list_of_tiny_objects_is_refused'),
    ('openai-test-button-takes-the-images-budget', O_PROBE, 'get_json("/models", crate::http::MODELS_REPLY)', 'get_json("/models", crate::http::IMAGES_REPLY)', O, 'reply_size', 'a_model_list_of_tiny_objects_is_refused'),
    ('openai-models-byte-cap-loosened', O_HTTP, 'pub(crate) const MODELS_REPLY: ReplyLimit = ReplyLimit {\n    bytes: 8 * 1024 * 1024,', 'pub(crate) const MODELS_REPLY: ReplyLimit = ReplyLimit {\n    bytes: 16 * 1024 * 1024,', O, 'reply_size', 'a_model_list_has_a_byte_cap_of_its_own'),
    ('openai-parse-budget-ignored', O_HTTP, 'limit.parsed,', UNLIMITED + ',', O, 'reply_size', 'a_chat_reply_of_tiny_objects_is_refused'),
    ('openai-chat-takes-the-images-budget', O_JOBS, 'read_json(resp, crate::http::CHAT_REPLY)', 'read_json(resp, crate::http::IMAGES_REPLY)', O, 'reply_size', 'a_chat_reply_of_tiny_objects_is_refused'),
    ('openai-chat-budget-loosened', O_HTTP, 'pub(crate) const CHAT_REPLY: ReplyLimit = ReplyLimit {\n    bytes: 32 * 1024 * 1024,\n    parsed: 64 * 1024 * 1024,', 'pub(crate) const CHAT_REPLY: ReplyLimit = ReplyLimit {\n    bytes: 32 * 1024 * 1024,\n    parsed: 160 * 1024 * 1024,', O, 'reply_size', 'a_chat_reply_of_tiny_objects_is_refused'),
    ('openai-structured-reparse-unbounded', O_CHAT, 'crate::http::CHAT_REPLY.parsed,', UNLIMITED + ',', O, 'reply_size', 'structured_output_of_tiny_objects_is_refused'),
    ('openai-stream-line-unbounded', O_STREAM, 'pub(crate) const MAX_SSE_LINE_PARSED: usize = 4 * MAX_SSE_LINE;', 'pub(crate) const MAX_SSE_LINE_PARSED: usize = ' + UNLIMITED + ';', O, 'reply_size', 'a_stream_line_of_tiny_objects_is_refused'),
    ('openai-images-budget-unbounded', O_HTTP, '    bytes: 128 * 1024 * 1024,\n    parsed: 160 * 1024 * 1024,', '    bytes: 128 * 1024 * 1024,\n    parsed: ' + UNLIMITED + ',', O, 'reply_size', 'an_image_reply_of_tiny_objects_is_refused'),
    ('openai-images-budget-below-a-picture', O_HTTP, '    bytes: 128 * 1024 * 1024,\n    parsed: 160 * 1024 * 1024,', '    bytes: 128 * 1024 * 1024,\n    parsed: 16 * 1024 * 1024,', O, 'reply_size', 'a_large_picture_still_arrives'),
    # Gemini, Vertex and declarative routes: one budget each.
    ('gemini-parse-unbounded', 'crates/ai-gemini/src/lib.rs', 'parse(&bytes, MAX_REPLY_PARSED)', 'parse(&bytes, ' + UNLIMITED + ')', 'craft-ai-gemini', 'worker_and_errors', 'a_reply_of_tiny_objects_is_refused'),
    ('vertex-parse-unbounded', 'crates/ai-vertex/src/lib.rs', 'parse(&bytes, MAX_REPLY_PARSED)', 'parse(&bytes, ' + UNLIMITED + ')', 'craft-ai-vertex', 'provider', 'a_reply_of_tiny_objects_is_refused'),
    ('declarative-parse-unbounded', 'crates/ai-declarative/src/http.rs', 'parse(&bytes, MAX_REPLY_PARSED)', 'parse(&bytes, ' + UNLIMITED + ')', 'craft-ai-declarative', 'direct', 'a_reply_of_tiny_objects_is_refused'),
    # ComfyUI: ordinary replies and the node list.
    ('comfyui-small-reply-unbounded', COMFY, 'const SMALL_REPLY_PARSED: usize = 16 * 1024 * 1024;', 'const SMALL_REPLY_PARSED: usize = ' + UNLIMITED + ';', 'craft-ai-comfyui', 'reply_size', 'a_small_reply_of_tiny_objects_is_refused'),
    ('comfyui-small-reply-takes-the-node-list-budget', COMFY, 'self.get_json_bounded(path, SMALL_REPLY, SMALL_REPLY_PARSED)', 'self.get_json_bounded(path, SMALL_REPLY, NODE_LIST_PARSED)', 'craft-ai-comfyui', 'reply_size', 'a_small_reply_of_tiny_objects_is_refused'),
    ('comfyui-node-list-unbounded', COMFY, 'const NODE_LIST_PARSED: usize = 256 * 1024 * 1024;', 'const NODE_LIST_PARSED: usize = ' + UNLIMITED + ';', 'craft-ai-comfyui', 'reply_size', 'a_node_list_of_tiny_objects_is_refused'),
]
