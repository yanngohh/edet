//! **The model a person is played by.**
//!
//! [`Anthropic`] speaks the Messages API: `POST /v1/messages` with the
//! `anthropic-version` header, tools declared as `name` / `description` /
//! `input_schema`, `tool_use` blocks back and `tool_result` blocks in the next
//! user message, and `cache_control` breakpoints so a person's growing life is
//! read from the prompt cache rather than again at full price. The credential
//! is `ANTHROPIC_API_KEY`, or a file named on the command line.
//!
//! [`OpenAi`] speaks the OpenAI chat-completions dialect: a model on this
//! machine — Ollama, llama.cpp's server, vLLM — so a run can be played for
//! nothing per day, or a hosted model that speaks the same dialect, which is
//! how a run reaches anything that is not the Anthropic API. The conversation is kept in
//! the Anthropic block shape the tape holds, whatever plays the run, and this
//! backend converts at the moment it calls and back again, so the player, the
//! report and the queue read one format.
//!
//! [`Scripted`] calls nothing. It looks at its own offers, signs what waits for
//! it, now and then offers a loan to somebody new or to another member, says
//! something on the square, and ends its day: enough to drive every path the
//! world has without spending anything. Its tape is not a run.

use serde_json::{json, Value};

use crate::event::Usage;

pub struct Request<'a> {
    pub system: &'a [Value],
    pub messages: &'a [Value],
    pub tools: &'a [Value],
    pub max_tokens: u32,
    pub tool_choice: Option<Value>,
}

pub struct Response {
    pub content: Vec<Value>,
    pub stop_reason: String,
    pub usage: Usage,
}

#[derive(Debug)]
pub enum CallError {
    /// The conversation no longer fits the model's context.
    ContextFull(String),
    /// A refusal that a wait would have fixed and every wait there was did
    /// not: a limit, an overload, a gateway, still standing when the patience
    /// ran out. The day is lost to the endpoint and not to the person, and
    /// the run may try it again later in the tick.
    Weather(String),
    Failed(String),
}

/// **The tokens a minute this process has agreed to send**, shared by every
/// worker. A call asks for room before it goes, and waits here — where the
/// wait costs a minute — rather than being refused there, where it costs a
/// person their day.
///
/// A sliding minute of what was sent, each entry the estimate the call went
/// out under and then, once the endpoint has counted it, what it counted. A
/// call that is refused is still in the window under its estimate: the
/// endpoint's limiter did not count it, but the next call has no way to know
/// whether this one went through, and erring on the full side is what keeps
/// the window under the limit.
pub struct Governor {
    limit: u64,
    window: std::sync::Mutex<std::collections::VecDeque<(std::time::Instant, u64, u64)>>,
    next: std::sync::atomic::AtomicU64,
}

static GOVERNOR: std::sync::OnceLock<Governor> = std::sync::OnceLock::new();

/// The process's one governor, at this limit; `None` for no limit. The first
/// caller's limit stands, which is one run's one configuration.
pub fn governor(tokens_per_minute: u64) -> Option<&'static Governor> {
    (tokens_per_minute > 0).then(|| {
        GOVERNOR.get_or_init(|| Governor {
            limit: tokens_per_minute,
            window: std::sync::Mutex::new(std::collections::VecDeque::new()),
            next: std::sync::atomic::AtomicU64::new(0),
        })
    })
}

impl Governor {
    const MINUTE: std::time::Duration = std::time::Duration::from_secs(60);

    /// Room for `tokens`, waited for. Returns the ticket `settle` takes.
    pub fn admit(&self, tokens: u64) -> u64 {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        loop {
            let wait = {
                let mut w = self.window.lock().expect("the governor's lock");
                let now = std::time::Instant::now();
                while w.front().is_some_and(|(at, _, _)| now.duration_since(*at) >= Self::MINUTE) {
                    w.pop_front();
                }
                let used: u64 = w.iter().map(|(_, _, n)| n).sum();
                // A call bigger than the whole limit goes when the window is
                // empty, or it would never go at all.
                if used + tokens <= self.limit || w.is_empty() {
                    w.push_back((now, id, tokens));
                    return id;
                }
                let (oldest, _, _) = w.front().expect("a non-empty window has a front");
                Self::MINUTE.saturating_sub(now.duration_since(*oldest)) + std::time::Duration::from_millis(spread())
            };
            std::thread::sleep(wait);
        }
    }

    /// What the endpoint counted for a ticket, replacing the estimate it went
    /// out under. An entry already out of the window is nothing to correct.
    pub fn settle(&self, ticket: u64, counted: u64) {
        let mut w = self.window.lock().expect("the governor's lock");
        if let Some(entry) = w.iter_mut().find(|(_, id, _)| *id == ticket) {
            entry.2 = counted;
        }
    }

    /// What the window holds now, for a test and a log line.
    pub fn in_flight(&self) -> u64 {
        let w = self.window.lock().expect("the governor's lock");
        w.iter().map(|(_, _, n)| n).sum()
    }
}

/// **The seconds a refusal's own words ask for**, where the header said
/// nothing. OpenAI's `429` says "Please try again in 10.666s" — or "859ms" —
/// in the body and sends no `retry-after` for a token limit, so without this a
/// doubling from one second stood in for a wait the endpoint had named.
pub fn asked_in_body(text: &str) -> Option<u64> {
    let low = text.to_ascii_lowercase();
    let at = low.find("try again in ")? + "try again in ".len();
    let rest = &low[at..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let n: f64 = digits.parse().ok()?;
    let unit = rest[digits.len()..].trim_start();
    let secs = if unit.starts_with("ms") {
        n / 1000.0
    } else if unit.starts_with('s') {
        n
    } else if unit.starts_with('m') {
        n * 60.0
    } else {
        return None;
    };
    (secs.is_finite() && secs >= 0.0).then(|| (secs.ceil() as u64).max(1))
}

pub trait Model {
    fn call(&mut self, req: &Request) -> Result<Response, CallError>;
    /// The backend and model, as the manifest records them.
    fn name(&self) -> String;
}

/// What the configuration adds to a body, written last so it wins.
///
/// A field this crate does not model is the endpoint's business — a slower
/// half-price tier, a switch for a model that reasons before it answers — and
/// naming it in the configuration keeps it in the manifest, where a later
/// reader can see which run they are holding.
fn merge(body: &mut Value, extra: &serde_json::Map<String, Value>) {
    if let Some(map) = body.as_object_mut() {
        for (k, v) in extra {
            map.insert(k.clone(), v.clone());
        }
    }
}

/// What a wait might fix, and what it cannot.
///
/// A rate limit, an overloaded endpoint or a gateway between here and it is
/// weather and is waited out. A key that is wrong, a body that is malformed
/// or a life too long for the model is an answer, and is returned at once —
/// a life too long even where a local server reports it as a 500, which is
/// why the BODY is read before this is asked.
fn worth_waiting(status: u16, body: &str) -> bool {
    come_back_later(status) && !says_too_long(status, body)
}

/// The statuses that mean come back later rather than do not come back.
fn come_back_later(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// Whether what came back says the life was too long for the model.
///
/// Two lists, because the two ways of saying it are not equally sure.
///
/// **What names the model's own context** is believed whatever the status,
/// since a local server reports a life it cannot hold as a 500 as readily as
/// a 400. **The looser way of saying it** — too long, too large, too many
/// tokens — is the vocabulary of gateways as much as of models: a 414 is a
/// URI too long, a 413 is a payload too large, a 504's page says the upstream
/// took too long. It is believed only where the status already says the
/// REQUEST was refused, and never where it says come back later, or a person
/// who was merely rate-limited is retired for somebody else's sentence.
///
/// Neither list holds the bare word `context`: a proxy's "context deadline
/// exceeded" is a timeout and an "invalid security context" is a key.
fn says_too_long(status: u16, body: &str) -> bool {
    const NAMES_THE_CONTEXT: [&str; 12] = [
        "context length",
        "context window",
        "context size",
        "context limit",
        "context overflow",
        "context is full",
        "context exhaust",
        "ran out of context",
        "exceeds the context",
        "exceed the context",
        "maximum context",
        "available context",
    ];
    let low = body.to_ascii_lowercase();
    if NAMES_THE_CONTEXT.iter().any(|phrase| low.contains(phrase)) {
        return true;
    }
    !come_back_later(status) && says_it_loosely(body)
}

/// The vocabulary a gateway shares with a model: a 414 is a URI too long, a
/// 413 a payload too large. Believed where the request was refused outright,
/// or where waiting it out changed nothing.
fn says_it_loosely(body: &str) -> bool {
    const SAYS_IT_LOOSELY: [&str; 3] = ["too long", "too large", "too many tokens"];
    let low = body.to_ascii_lowercase();
    SAYS_IT_LOOSELY.iter().any(|phrase| low.contains(phrase))
}

/// How long to wait before trying again: what the endpoint asked for, capped
/// at a minute because a wait longer than that is a decision for a person, or
/// else a doubling from one second. Either way it is spread, because the case
/// this exists for is many workers refused at the same instant by one shared
/// limit — and told, all of them, to come back at the same instant.
fn backoff(attempt: u32, asked: Option<u64>) -> std::time::Duration {
    let base = match asked {
        Some(secs) => secs.min(60) * 1000,
        None => (1u64 << attempt.min(5)) * 1000,
    };
    std::time::Duration::from_millis(base + spread())
}

/// Up to half a second of difference between two workers, taken from the
/// clock and the thread, since two threads refused in the same millisecond
/// would otherwise wait exactly as long as each other.
fn spread() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut h);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    (h.finish() ^ now) % 500
}

/// The seconds an endpoint asked to be left alone for. A date rather than a
/// count of seconds — which the standard allows and neither endpoint here
/// sends — reads as no answer at all, and the doubling stands in for it.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let secs = headers.get("retry-after")?.to_str().ok()?.trim().parse::<f64>().ok()?;
    (secs.is_finite() && secs > 0.0).then(|| secs.ceil() as u64)
}

/// What an endpoint answered, and whether the answer outlasted every wait.
///
/// **Weather clears.** A 500 still standing after all the patience there was
/// is the server's own error and not a moment's trouble, so a body that says
/// a life is too long — in the loose words a gateway also uses — is believed
/// at that point and nowhere earlier: a local server reporting an overflow
/// that way is answered by retiring the person rather than by asking it again
/// tomorrow and every day after.
///
/// It is 500 and not every retryable status, because 429, 502, 503 and 504
/// say WHO is refusing: a limit, a gateway, an endpoint that is loading. Only
/// the first of them is the model's own server answering about this request,
/// and only it should be able to end a person on a loose sentence.
struct Answered {
    status: reqwest::StatusCode,
    text: String,
    waited_out: bool,
}

/// One call, tried again while what refused it is weather and there is
/// patience left.
///
/// **A timeout is not weather.** The same body given to the same endpoint
/// takes the same time to generate, so resending it spends `timeout_secs`
/// again for the same answer; it is returned as the failure it is, and the
/// person's day is a silence. What is retried is a refusal that arrived
/// QUICKLY — a limit, an overload, a gateway — or a connection that never
/// opened.
///
/// **Patience is in seconds, not in tries**, because tries alone bound
/// nothing: five attempts at a ten-minute timeout is fifty minutes for one
/// call, and a day of twenty-four calls is a day of wall clock. No wait is
/// begun that would take the call past its patience.
fn with_patience(
    retries: u32,
    patience: std::time::Duration,
    send: impl Fn(u32) -> reqwest::Result<reqwest::blocking::Response>,
) -> Result<Answered, CallError> {
    let start = std::time::Instant::now();
    let mut attempt = 0u32;
    loop {
        match send(attempt) {
            Ok(resp) => {
                let status = resp.status();
                let asked = retry_after(resp.headers());
                let text = match resp.text() {
                    Ok(text) => text,
                    // A body that stopped arriving is the same weather as a
                    // connection that dropped, and is waited out the same way.
                    Err(e) => {
                        let wait = backoff(attempt, asked);
                        if e.is_timeout() || attempt >= retries || start.elapsed() + wait >= patience {
                            return Err(CallError::Failed(format!("the response could not be read: {e}")));
                        }
                        attempt += 1;
                        std::thread::sleep(wait);
                        continue;
                    }
                };
                if status.is_success() || !worth_waiting(status.as_u16(), &text) {
                    return Ok(Answered { status, text, waited_out: false });
                }
                if attempt >= retries {
                    return Ok(Answered { status, text, waited_out: true });
                }
                let wait = backoff(attempt, asked.or_else(|| asked_in_body(&text)));
                if start.elapsed() + wait >= patience {
                    return Ok(Answered { status, text, waited_out: true });
                }
                attempt += 1;
                std::thread::sleep(wait);
            }
            Err(e) => {
                let wait = backoff(attempt, None);
                if e.is_timeout() || attempt >= retries || start.elapsed() + wait >= patience {
                    return Err(CallError::Failed(format!("the request did not complete: {e}")));
                }
                attempt += 1;
                std::thread::sleep(wait);
            }
        }
    }
}

pub struct Anthropic {
    client: reqwest::blocking::Client,
    key: String,
    model: String,
    retries: u32,
    extra: serde_json::Map<String, Value>,
    /// What the model can hold, counted before the call is made. Zero is no
    /// bound of ours; a number is the one check that does not depend on the
    /// endpoint's English.
    context_tokens: u64,
    /// How long a call may spend WAITING to be tried again, over all its
    /// attempts: one timeout's worth, so a call is bounded by about two.
    patience: std::time::Duration,
    governor: Option<&'static Governor>,
}

impl Anthropic {
    pub fn new(
        model: &str,
        timeout_secs: u64,
        key_file: Option<&str>,
        retries: u32,
        context_tokens: u64,
        extra: serde_json::Map<String, Value>,
        tokens_per_minute: u64,
    ) -> Result<Anthropic, String> {
        let key = match key_file {
            Some(path) => std::fs::read_to_string(path)
                .map_err(|e| format!("{path}: {e}"))?
                .trim()
                .to_string(),
            None => std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| "no credential: set ANTHROPIC_API_KEY or pass --api-key-file".to_string())?,
        };
        if key.is_empty() {
            return Err("the credential is empty".into());
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Anthropic {
            client,
            key,
            model: model.to_string(),
            retries,
            extra,
            context_tokens,
            patience: std::time::Duration::from_secs(timeout_secs),
            governor: governor(tokens_per_minute),
        })
    }
}

/// A refusal that outlasted the patience, as the two backends both read it:
/// weather that did not clear is the endpoint's failure and not the person's,
/// and the run may live the day again later.
fn still_weather(status: reqwest::StatusCode, waited_out: bool, message: &str) -> Option<CallError> {
    (waited_out && come_back_later(status.as_u16())).then(|| CallError::Weather(format!("{status}: {message}")))
}

impl Anthropic {
    /// One call, tried again while what refused it is weather: a rate limit,
    /// an overloaded endpoint, a gateway, a request that never came back. The
    /// wait is what the endpoint asked for, or a doubling.
    fn ask(&self, body: &Value) -> Result<Answered, CallError> {
        let send = |_: u32| {
            self.client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(body)
                .send()
        };
        with_patience(self.retries, self.patience, send)
    }
}

impl Model for Anthropic {
    fn call(&mut self, req: &Request) -> Result<Response, CallError> {
        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "system": req.system,
            "messages": req.messages,
            "tools": req.tools,
        });
        if let Some(choice) = &req.tool_choice {
            body["tool_choice"] = choice.clone();
        }
        merge(&mut body, &self.extra);
        if let Some(full) = too_long_already(&body, req.max_tokens, self.context_tokens) {
            return Err(full);
        }
        let ticket = self.governor.map(|g| g.admit(rough_of(&body) + u64::from(req.max_tokens)));
        let Answered { status, text, waited_out } = self.ask(&body)?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|_| CallError::Failed(format!("{status}: {}", text.chars().take(300).collect::<String>())))?;
        if !status.is_success() {
            let message = v["error"]["message"].as_str().unwrap_or("").to_string();
            let kind = v["error"]["type"].as_str().unwrap_or("").to_string();
            // The same rule the loop reads, and the same one the local
            // backend reads: a life too long is what retires a person, and it
            // is decided in one place.
            // Weather clears; a refusal that outlasted every wait does not.
            if says_too_long(status.as_u16(), &message)
                || (waited_out && status.as_u16() == 500 && says_it_loosely(&message))
            {
                return Err(CallError::ContextFull(message));
            }
            if let Some(weather) = still_weather(status, waited_out, &message) {
                return Err(weather);
            }
            return Err(CallError::Failed(format!("{status} {kind}: {message}")));
        }
        let u = &v["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        if let (Some(g), Some(t)) = (self.governor, ticket) {
            g.settle(
                t,
                n("input_tokens")
                    + n("cache_read_input_tokens")
                    + n("cache_creation_input_tokens")
                    + n("output_tokens"),
            );
        }
        Ok(Response {
            content: v["content"].as_array().cloned().unwrap_or_default(),
            stop_reason: v["stop_reason"].as_str().unwrap_or("").to_string(),
            usage: Usage {
                calls: 1,
                input_tokens: n("input_tokens"),
                output_tokens: n("output_tokens"),
                cache_read_input_tokens: n("cache_read_input_tokens"),
                cache_creation_input_tokens: n("cache_creation_input_tokens"),
            },
        })
    }

    fn name(&self) -> String {
        format!("anthropic/{}", self.model)
    }
}

/// A model on this machine, over an OpenAI-compatible chat-completions
/// endpoint.
pub struct OpenAi {
    client: reqwest::blocking::Client,
    url: String,
    key: Option<String>,
    model: String,
    /// What the endpoint calls the reply's token bound.
    tokens_field: String,
    retries: u32,
    extra: serde_json::Map<String, Value>,
    patience: std::time::Duration,
    /// What the model can hold, in tokens; zero for "no bound of ours".
    context_tokens: u64,
    calls: u64,
    governor: Option<&'static Governor>,
}

impl OpenAi {
    pub fn new(cfg: &crate::config::ModelConfig) -> Result<OpenAi, String> {
        let key = match cfg.api_key_env.trim() {
            "" => None,
            var => Some(std::env::var(var).map_err(|_| format!("model.api_key_env names {var}, which is not set"))?),
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(cfg.timeout_secs))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(OpenAi {
            client,
            url: format!("{}/chat/completions", cfg.base_url.trim_end_matches('/')),
            key,
            model: cfg.model.clone(),
            retries: cfg.retries,
            extra: cfg.extra_body.clone(),
            patience: std::time::Duration::from_secs(cfg.timeout_secs),
            tokens_field: match cfg.max_tokens_field.trim() {
                "" => "max_tokens".to_string(),
                named => named.to_string(),
            },
            context_tokens: cfg.context_tokens,
            calls: 0,
            governor: governor(cfg.tokens_per_minute),
        })
    }
}

/// The conversation as the chat-completions dialect takes it: a system message,
/// then user and assistant turns, with a tool call on the assistant's side and
/// a `tool` message per result.
fn to_openai(req: &Request) -> Vec<Value> {
    let text_of = |blocks: &[Value]| {
        blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let mut out = vec![json!({ "role": "system", "content": text_of(req.system) })];
    for m in req.messages {
        let blocks: Vec<Value> = match &m["content"] {
            Value::Array(a) => a.clone(),
            Value::String(s) => vec![json!({ "type": "text", "text": s })],
            _ => Vec::new(),
        };
        let role = m["role"].as_str().unwrap_or("user");
        // A tool result is its own message, and it must follow the call it
        // answers, so results are emitted before the text of the same turn.
        for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
            out.push(json!({
                "role": "tool",
                "tool_call_id": b["tool_use_id"],
                "content": match &b["content"] {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                },
            }));
        }
        let calls: Vec<Value> = blocks
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| {
                json!({
                    "id": b["id"],
                    "type": "function",
                    "function": { "name": b["name"], "arguments": b["input"].to_string() },
                })
            })
            .collect();
        let said = text_of(&blocks);
        if !calls.is_empty() {
            let mut msg = json!({ "role": "assistant", "tool_calls": calls });
            if !said.is_empty() {
                msg["content"] = json!(said);
            }
            out.push(msg);
        } else if !said.is_empty() {
            out.push(json!({ "role": role, "content": said }));
        }
    }
    out
}

/// The tools as the dialect declares them.
fn to_openai_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t["description"],
                    "parameters": t["input_schema"],
                },
            })
        })
        .collect()
}

/// A reply, back in the block shape the tape keeps.
fn from_openai(message: &Value, id: u64) -> Vec<Value> {
    let mut content = Vec::new();
    if let Some(text) = message["content"].as_str().filter(|t| !t.trim().is_empty()) {
        content.push(json!({ "type": "text", "text": text }));
    }
    for (n, call) in message["tool_calls"].as_array().into_iter().flatten().enumerate() {
        let raw = call["function"]["arguments"].clone();
        // The dialect passes arguments as a string of JSON; some servers pass
        // the object itself. Both are read, and neither is guessed at.
        let input = match &raw {
            Value::String(s) => serde_json::from_str(s).unwrap_or(json!({})),
            Value::Object(_) => raw.clone(),
            _ => json!({}),
        };
        let call_id = call["id"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("call_{id}_{n}"));
        content.push(json!({
            "type": "tool_use",
            "id": call_id,
            "name": call["function"]["name"],
            "input": input,
        }));
    }
    content
}

/// Four characters to the token, which is the rule of thumb a local endpoint's
/// own tokeniser is not worth a round trip to improve on.
/// A life counted before it is sent, against what the model can hold: the
/// check that needs no wording from anybody. Zero asks nothing.
///
/// What is counted is what is POSTED — the local backend sends a body of a
/// different shape from the one it was handed — plus the room the reply is
/// promised, since a window holds the question and the answer together.
fn too_long_already(sending: &Value, max_tokens: u32, context_tokens: u64) -> Option<CallError> {
    let rough = rough_of(sending) + u64::from(max_tokens);
    (context_tokens > 0 && rough > context_tokens)
        .then(|| CallError::ContextFull(format!("about {rough} tokens against a context of {context_tokens}")))
}

/// Four characters a token, over what is actually on the wire.
fn rough_of(sending: &Value) -> u64 {
    (sending.to_string().len() / 4) as u64
}

impl OpenAi {
    /// One call, tried again while what refused it is weather. A local server
    /// says 503 while it loads a model and 429 when its queue is full, and
    /// both are answered by waiting rather than by losing a person's day.
    fn ask(&self, body: &Value) -> Result<Answered, CallError> {
        let send = |_: u32| {
            let mut request = self.client.post(&self.url).header("content-type", "application/json");
            if let Some(key) = &self.key {
                request = request.header("authorization", format!("Bearer {key}"));
            }
            request.json(body).send()
        };
        with_patience(self.retries, self.patience, send)
    }
}

impl Model for OpenAi {
    fn call(&mut self, req: &Request) -> Result<Response, CallError> {
        self.calls += 1;
        let mut body = json!({
            "model": self.model,
            "messages": to_openai(req),
            "tools": to_openai_tools(req.tools),
            "stream": false,
        });
        body[&self.tokens_field] = json!(req.max_tokens);
        if let Some(choice) = &req.tool_choice {
            // The dialect names a forced tool the other way round.
            body["tool_choice"] = json!({ "type": "function", "function": { "name": choice["name"] } });
        }
        merge(&mut body, &self.extra);
        if let Some(full) = too_long_already(&body, req.max_tokens, self.context_tokens) {
            return Err(full);
        }
        // The limiter counts the prompt and the reply it may write; so does
        // the estimate the call goes out under.
        let ticket = self.governor.map(|g| g.admit(rough_of(&body) + u64::from(req.max_tokens)));
        let Answered { status, text, waited_out } = self.ask(&body)?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|_| CallError::Failed(format!("{status}: {}", text.chars().take(300).collect::<String>())))?;
        if !status.is_success() {
            let message = v["error"]["message"].as_str().unwrap_or(&text).to_string();
            // One rule for both: what the loop refuses to retry as a life too
            // long is what retires the person, and nothing else does.
            // Weather clears; a refusal that outlasted every wait does not.
            if says_too_long(status.as_u16(), &message)
                || (waited_out && status.as_u16() == 500 && says_it_loosely(&message))
            {
                return Err(CallError::ContextFull(message));
            }
            if let Some(weather) = still_weather(status, waited_out, &message) {
                return Err(weather);
            }
            return Err(CallError::Failed(format!("{status}: {message}")));
        }
        if let (Some(g), Some(t)) = (self.governor, ticket) {
            let n = |k: &str| v["usage"][k].as_u64().unwrap_or(0);
            g.settle(t, n("prompt_tokens") + n("completion_tokens"));
        }
        let choice = &v["choices"][0];
        let content = from_openai(&choice["message"], self.calls);
        if content.is_empty() {
            return Err(CallError::Failed("the reply held neither words nor a tool call".into()));
        }
        let n = |k: &str| v["usage"][k].as_u64().unwrap_or(0);
        // This dialect reports a cache hit inside the prompt's own count —
        // `prompt_tokens` is the whole prompt and `cached_tokens` the part of
        // it that was already there — where the other dialect reports the two
        // apart. Counted as they come, a cached token would be billed at the
        // fresh rate, and the cache that a run's whole price rests on would be
        // invisible in the one figure that measures it.
        let cached = v["usage"]["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
        Ok(Response {
            content,
            stop_reason: choice["finish_reason"].as_str().unwrap_or("").to_string(),
            usage: Usage {
                calls: 1,
                input_tokens: n("prompt_tokens").saturating_sub(cached),
                cache_read_input_tokens: cached,
                output_tokens: n("completion_tokens"),
                ..Default::default()
            },
        })
    }

    fn name(&self) -> String {
        format!("openai/{}", self.model)
    }
}

/// A fixed policy over the same tools, for exercising the apparatus. It keeps
/// nothing between calls: what it does next is read off the conversation it is
/// handed, which is what makes a scripted tape the same tape whether it was
/// lived in one sitting or five.
pub struct Scripted;

fn tool_use(id: u64, name: &str, input: Value) -> Response {
    Response {
        content: vec![
            json!({ "type": "tool_use", "id": format!("toolu_scripted_{id}"), "name": name, "input": input }),
        ],
        stop_reason: "tool_use".into(),
        usage: Usage { calls: 1, ..Default::default() },
    }
}

fn digest_of(s: &str) -> u64 {
    let d = edet_state::root::value_digest(s.as_bytes());
    u64::from_be_bytes(d[..8].try_into().expect("eight bytes"))
}

impl Model for Scripted {
    fn call(&mut self, req: &Request) -> Result<Response, CallError> {
        // Where the call sits in the conversation, not how many this PROCESS
        // has made: a run of sixty days in one sitting and the same run in
        // three wrote tapes differing in nothing but these ids, and a tape
        // that depends on when somebody stopped for lunch is not the tape of
        // the run. A day's messages only grow, so no two calls of a day share
        // one.
        let id = req.messages.len() as u64;
        if let Some(choice) = &req.tool_choice {
            if choice["name"] == "write_card" {
                return Ok(tool_use(
                    id,
                    "write_card",
                    json!({
                        "name": "neighbour",
                        "card": "You fix bicycles from a shed behind your house. Most of your customers pay when they collect, and a few pay when they can.",
                        "tier": "wallet",
                    }),
                ));
            }
        }
        // Where in the day this is: a note opens it, the offers come back
        // next, and whatever follows the one act it takes ends it.
        let last = req.messages.last().cloned().unwrap_or(Value::Null);
        let blocks = last["content"].as_array().cloned().unwrap_or_default();
        let opens = blocks
            .iter()
            .any(|b| b["type"] == "text" && b["text"].as_str().is_some_and(|t| t.starts_with("Day ")));
        let previous = req
            .messages
            .iter()
            .rev()
            .nth(1)
            .and_then(|m| m["content"].as_array())
            .and_then(|c| c.iter().find_map(|b| b["name"].as_str().map(str::to_string)));
        let who = digest_of(&req.system.last().map(|v| v.to_string()).unwrap_or_default());
        if opens {
            return Ok(tool_use(id, "offers", json!({})));
        }
        let result = blocks.iter().find_map(|b| b["content"].as_str().map(str::to_string));
        if previous.as_deref() == Some("my_account") {
            let account: Value = result
                .as_deref()
                .and_then(|r| serde_json::from_str(r).ok())
                .unwrap_or(Value::Null);
            if let Some(c) = account["account"]["owes"]
                .as_array()
                .and_then(|o| o.first())
                .and_then(|c| c["id"].as_u64())
            {
                return Ok(tool_use(
                    id,
                    "set_instruction",
                    json!({ "kind": "pay_at_maturity", "contract": c, "paid_with": "cash" }),
                ));
            }
        }
        if previous.as_deref() != Some("offers") {
            return Ok(tool_use(id, "end_day", json!({})));
        }
        let offers: Value = result
            .as_deref()
            .and_then(|r| serde_json::from_str(r).ok())
            .unwrap_or(Value::Null);
        if let Some(r) = offers["awaiting_me"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e["ref"].as_str())
        {
            return Ok(tool_use(id, "sign_offer", json!({ "ref": r })));
        }
        Ok(match digest_of(&format!("{who}/{}", req.messages.len())) % 6 {
            0 => tool_use(
                id,
                "offer_credit",
                json!({ "counterparty": "new", "you_are": "lender", "amount": "20.00", "days": 30 }),
            ),
            1 => tool_use(
                id,
                "offer_credit",
                json!({ "counterparty": (who % 8).to_string(), "you_are": "lender", "amount": "15.00", "days": 30 }),
            ),
            2 => tool_use(id, "post", json!({ "text": "Good morning, everybody." })),
            3 => tool_use(id, "set_instruction", json!({ "kind": "accept_payments" })),
            4 => tool_use(id, "my_account", json!({})),
            _ => tool_use(id, "diary", json!({ "text": "An ordinary day." })),
        })
    }

    fn name(&self) -> String {
        "scripted".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_s_own_words_name_the_wait() {
        let openai = "Rate limit reached for gpt-5.6-luna in organization org-x on tokens per min (TPM): Limit 500000, \
                      Used 498917, Requested 89972. Please try again in 10.666s. Visit https://platform.openai.com/account/rate-limits to learn more.";
        assert_eq!(asked_in_body(openai), Some(11));
        assert_eq!(asked_in_body("Please try again in 859ms."), Some(1));
        assert_eq!(asked_in_body("Please try again in 37ms."), Some(1));
        assert_eq!(asked_in_body("Please try again in 2m."), Some(120));
        assert_eq!(asked_in_body("overloaded"), None);
        assert_eq!(asked_in_body("try again in soon"), None);
    }

    #[test]
    fn the_wait_is_what_was_asked_and_capped_at_a_minute() {
        assert!(backoff(0, Some(11)) >= std::time::Duration::from_secs(11));
        assert!(backoff(0, Some(11)) < std::time::Duration::from_millis(11_500));
        assert!(backoff(9, Some(600)) <= std::time::Duration::from_millis(60_500));
        assert!(backoff(3, None) >= std::time::Duration::from_secs(8));
    }

    #[test]
    fn the_governor_admits_up_to_its_limit_and_settles_to_what_was_counted() {
        let g = Governor {
            limit: 100,
            window: std::sync::Mutex::new(std::collections::VecDeque::new()),
            next: std::sync::atomic::AtomicU64::new(0),
        };
        let a = g.admit(60);
        let b = g.admit(30);
        assert_eq!(g.in_flight(), 90);
        g.settle(a, 50);
        assert_eq!(g.in_flight(), 80);
        // Room for twenty more goes at once; the settled figure is what counts.
        let started = std::time::Instant::now();
        g.admit(20);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(g.in_flight(), 100);
        g.settle(b, 0);
        assert_eq!(g.in_flight(), 70);
        // A ticket already gone is nothing to correct.
        g.settle(999, 5);
        assert_eq!(g.in_flight(), 70);
    }

    #[test]
    fn a_call_bigger_than_the_limit_goes_when_the_window_is_empty() {
        let g = Governor {
            limit: 10,
            window: std::sync::Mutex::new(std::collections::VecDeque::new()),
            next: std::sync::atomic::AtomicU64::new(0),
        };
        let started = std::time::Instant::now();
        g.admit(50);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(g.in_flight(), 50);
    }

    #[test]
    fn weather_that_outlasted_the_patience_is_named_as_weather() {
        let s = |n: u16| reqwest::StatusCode::from_u16(n).unwrap();
        assert!(matches!(still_weather(s(429), true, "limit"), Some(CallError::Weather(_))));
        assert!(matches!(still_weather(s(503), true, "busy"), Some(CallError::Weather(_))));
        assert!(still_weather(s(429), false, "limit").is_none());
        assert!(still_weather(s(401), true, "no key").is_none());
    }
}
