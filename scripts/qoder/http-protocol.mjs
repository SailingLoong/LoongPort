// Derived from magpie-community/plugins cdf45137, @magpie-community/opencode-qoder-auth 0.2.18.
// Protocol ancestry: CLIProxyAPI Qoder support (MIT). See both bundled licenses.
// Extracted from reviewed rev3; auth/store/refresh/device/campaign/SDK code is absent.
import {createCipheriv,createHash,publicEncrypt,randomBytes,randomUUID,constants} from "node:crypto";
import {STATUS_CODES} from "node:http";

// Frozen rev3 source lines 44-46.
const CHAT_PATH = "/algo/api/v2/service/pro/sse/agent_chat_generation?FetchKeys=llm_model_result&AgentId=agent_common&Encode=1"
const MODELS_PATH = "/algo/api/v2/model/list?Encode=1"
const COSY_VERSION = "1.1.49"

// Frozen rev3 source lines 56-161.
const RSA_KEY = `-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDA8iMH5c02LilrsERw9t6Pv5Nc
4k6Pz1EaDicBMpdpxKduSZu5OANqUq8er4GM95omAGIOPOh+Nx0spthYA2BqGz+l
6HRkPJ7S236FZz73In/KVuLnwI8JJ2CbuJap8kvheCCZpmAWpb/cPx/3Vr/J6I17
XcW+ML9FoCI6AOvOzwIDAQAB
-----END PUBLIC KEY-----`

const QODER_SYS = "You are a Qoder agent. Use the instructions below and the tools available to you to assist the user."

const hexID = () => randomUUID().replaceAll("-", "")

// ---- the COSY envelope --------------------------------------------------------

// machineOS names the machine as Qoder's desktop clients do.
function machineOS() {
  const arch = { x64: "x86_64", ia32: "x86", arm64: "aarch64" }[process.arch] ?? process.arch
  return arch + "_" + process.platform
}

// userBlob is the account as the envelope carries it: AES-128-CBC under a
// fresh key that is its own IV, the key wrapped with Qoder's RSA key.
function userBlob(u) {
  const raw = JSON.stringify({ uid: u.uid, aid: "", name: u.name ?? "", email: u.email ?? "", security_oauth_token: u.access })
  const key = Buffer.from(hexID().slice(0, 16))
  const c = createCipheriv("aes-128-cbc", key, key)
  const info = Buffer.concat([c.update(raw, "utf8"), c.final()]).toString("base64")
  const wrapped = publicEncrypt({ key: RSA_KEY, padding: constants.RSA_PKCS1_PADDING }, key).toString("base64")
  return { info, key: wrapped }
}

// cosyHeaders signs a call to url whose body, in its wire form, is body.
function cosyHeaders(url, u, body, ts = Math.floor(Date.now() / 1000)) {
  if (!u?.machineId) throw new Error("Qoder: the account has no machine id; sign in again")
  const cn = u.region === "cn"
  const version = cn ? "1.0.0" : COSY_VERSION
  const blob = userBlob(u)
  const payload = Buffer.from(
    JSON.stringify({ version: "v1", requestId: hexID(), info: blob.info, cosyVersion: version, ideVersion: "" }),
  ).toString("base64")
  let path = new URL(url).pathname
  if (path.startsWith("/algo")) path = path.slice(5)
  const sig = createHash("md5").update(`${payload}\n${blob.key}\n${ts}\n${body}\n${path}`).digest("hex")
  const m = u.machineId
  return {
    Accept: "application/json",
    "Accept-Encoding": "identity",
    "Content-Type": "application/json",
    Authorization: `Bearer COSY.${payload}.${sig}`,
    "Cosy-Business-Product": cn ? "cli" : "app",
    "Cosy-Business-Type": "agent",
    "Cosy-ClientIp": m,
    "Cosy-ClientType": cn ? "5" : "10",
    "Cosy-Data-Policy": "disagree",
    "Cosy-Date": String(ts),
    "Cosy-Key": blob.key,
    "Cosy-MachineId": m,
    "Cosy-MachineToken": m,
    "Cosy-MachineType": "5",
    "Cosy-MachineOS": machineOS(),
    "Cosy-Scene": "app",
    "Cosy-User": u.uid,
    "Cosy-Version": version,
    "Login-Version": "v2",
    ...(cn ? {"Cosy-Bodyhash": createHash("md5").update(body).digest("hex"), "Cosy-Bodylength": String(Buffer.byteLength(body)), "Cosy-Sigpath": path, "Cosy-Organization-Id": "", "Cosy-Organization-Tags": "", "X-Request-Id": randomUUID()} : {}),
  }
}

// ---- the body codec -----------------------------------------------------------

// Base64 through the client's shuffled alphabet ('$' pads), then the first
// and last thirds swapped.
const ALPHABET = "_doRTgHZBKcGVjlvpC,@aFSx#DPuNJme&i*MzLOEn)sUrthbf%Y^w.(kIQyXqWA!"

function segmentEncode(bytes) {
  let s = ""
  let acc = 0
  let nb = 0
  for (const b of bytes) {
    acc = ((acc << 8) | b) & 0xffffff
    nb += 8
    while (nb >= 6) {
      nb -= 6
      s += ALPHABET[(acc >> nb) & 63]
    }
  }
  if (nb > 0) s += ALPHABET[(acc << (6 - nb)) & 63]
  while (s.length % 4) s += "$"
  return s
}

function swapThirds(s) {
  const t = Math.floor(s.length / 3)
  return t ? s.slice(s.length - t) + s.slice(t, s.length - t) + s.slice(0, t) : s
}

const encodeBody = (text) => swapThirds(segmentEncode(Buffer.from(text, "utf8")))

function decodeBody(wire) {
  const s = swapThirds(wire)
  const out = []
  for (let g = 0; g + 4 <= s.length; g += 4) {
    const vals = [...s.slice(g, g + 4)].filter((c) => c !== "$").map((c) => ALPHABET.indexOf(c))
    let x = 0
    for (const v of vals) x = x * 64 + v
    const n = Math.floor((6 * vals.length) / 8)
    for (let k = 0; k < n; k++) out.push(Math.floor(x / 2 ** (6 * vals.length - 8 * (k + 1))) & 255)
  }
  return Buffer.from(out).toString("utf8")
}

// Frozen rev3 source lines 398-398.
const EFFORT_ORDER = ["minimal", "low", "medium", "high", "xhigh", "max"]

// Frozen rev3 source lines 431-579.
// freeOf reads whether a model costs the plan no credits: a price_factor
// (the credits a request costs, as a multiple; priceFactor in camel case)
// of 0, the price Qoder's own client shows ("0×"). is_free is no word on
// that: Qoder's listing has it true on Qwen3.8-Max at 0.5×, an off-peak
// discount (错峰 4 折) on it, and Qoder's client shows the price, not it.
// It is taken only from a listing with no price at all. A price of 0 for
// the while an active promotion lasts, with a price before it, is a
// discount, not a free model; a limited-time free model (Qwen3.8-Flash:
// 0×, its original_price_factor 0.1 struck through) is free while it is.
function freeOf(raw) {
  const price = raw.price_factor ?? raw.priceFactor
  if (typeof price === "number" && Number.isFinite(price)) {
    if (price !== 0) return false
    const p = raw.promotion ?? raw.prommotion
    const before = p?.before_promotion_price_factor ?? p?.beforePromotionPriceFactor
    return !(p?.active === true && typeof before === "number" && before > 0)
  }
  return (raw.is_free ?? raw.isFree) === true
}

// rateOf reads a model's price as Qoder's client shows it beside the model
// (magpie shows it in its lists, so the cheap ones can be picked without
// opening Qoder): rate, the price_factor (0.5×), and rateWas, the price
// struck through beside it while a discount runs: an active promotion's
// before_promotion_price_factor, else an original_price_factor above the
// price (Qwen3.8-Flash: 0×, 0.1× struck through). A promotion's 0 that
// isn't free is its price before times its discount_factor. A listing with
// no price gives none (0). The same rule as magpie's built-in
// (internal/qoder/models.go, ModelInfo.free).
function rateOf(raw) {
  const price = raw.price_factor ?? raw.priceFactor
  if (typeof price !== "number" || !Number.isFinite(price)) return { rate: 0, rateWas: 0 }
  const num = (v) => (typeof v === "number" && Number.isFinite(v) ? v : 0)
  const p = raw.promotion ?? raw.prommotion
  const before = p?.before_promotion_price_factor ?? p?.beforePromotionPriceFactor
  const discount = num(p?.discount_factor) || num(p?.discountFactor)
  const discounted = p?.active === true && typeof before === "number" && before > 0
  let rate = Math.max(price, 0)
  let rateWas = Math.max(num(raw.original_price_factor), num(raw.originalPriceFactor))
  if (discounted) {
    rateWas = before
    if (rate === 0) rate = before * discount
  }
  if (rateWas <= rate) rateWas = 0
  return { rate, rateWas }
}

// windowsOf reads the context windows a listing entry offers, smallest
// first, and the one Qoder's client picks unless asked for another:
// context_config's entries ({"200k": {token_count: 200000, is_default:
// true}, "1m": {token_count: 1000000}, …}), else available_context_windows,
// as Qoder's client reads them (qodercli's catalog). Qoder's Context
// setting (200K / 400K / 1M) offers them on Qwen3.8-Flash and the rest;
// max_input_tokens (180K there) is only what a request that names none
// gets.
function windowsOf(raw) {
  const n = (v) => (Number.isInteger(v) && v > 0 ? v : 0)
  const windows = []
  let def = 0
  const cc = raw?.context_config ?? raw?.contextConfig
  if (cc && typeof cc === "object" && !Array.isArray(cc)) {
    for (const e of Object.values(cc)) {
      const t = n(e?.token_count)
      if (!t) continue
      if (!windows.includes(t)) windows.push(t)
      if (e.is_default === true || e.is_default === 1 || e.is_default === "1") def = t
    }
  }
  if (!windows.length) {
    const list = raw?.available_context_windows ?? raw?.availableContextWindows
    for (const v of Array.isArray(list) ? list : []) if (n(v) && !windows.includes(v)) windows.push(v)
    def = n(raw?.default_context_window ?? raw?.defaultContextWindow)
    if (!windows.includes(def)) def = 0
  }
  windows.sort((a, b) => a - b)
  return { windows, defaultWindow: def }
}

// modelInfo reads one entry of the listing's "chat" array: its efforts and
// default from thinking_config (is_reasoning alone when it has none), and
// its context: the largest window it offers (windowsOf), since a request
// that needs more than the default is sent a larger one (windowFor).
function modelInfo(raw) {
  const { windows, defaultWindow } = windowsOf(raw)
  const m = {
    key: raw.key,
    source: raw.source ?? "",
    name: raw.display_name || raw.key,
    images: !!raw.is_vl,
    context: windows.length ? windows[windows.length - 1] : raw.max_input_tokens || 0,
    windows,
    defaultWindow,
    thinks: !!raw.is_reasoning,
    alwaysThinks: false,
    efforts: Array.isArray(raw.reasoning_efforts) ? [...raw.reasoning_efforts] : [],
    defaultEffort: "",
    free: freeOf(raw),
    ...rateOf(raw),
    config: raw,
  }
  const tc = raw.thinking_config
  if (tc && typeof tc === "object") {
    m.thinks = tc.enabled != null
    m.alwaysThinks = m.thinks && tc.disabled == null
    m.efforts = []
    if (m.thinks) {
      for (const [name, e] of Object.entries(tc.enabled.efforts ?? {})) {
        m.efforts.push(name)
        if (e?.is_default) m.defaultEffort = name
      }
      const rank = (s) => (EFFORT_ORDER.includes(s) ? EFFORT_ORDER.indexOf(s) : EFFORT_ORDER.length)
      m.efforts.sort((a, b) => rank(a) - rank(b) || (a < b ? -1 : a > b ? 1 : 0))
    }
  }
  return m
}

// sceneModels is the models an agent can pick from the listing: the "app"
// scene's, which is what Qoder's app shows and what a request names
// (qoderBody's product and session_type "app"), in its order, then any
// "chat" model the app no longer lists, after the model it followed there,
// so a pick made before stays served. On 2026-10-12 the app listed Fast
// (fast, 0.8×) where Performance (1.1×) had been, and chat still listed
// Performance (magpie-community/plugins#97). A listing with only one scene
// gives that one. Each key once: the app's entry wins.
function sceneModels(listing) {
  const app = Array.isArray(listing?.app) ? listing.app : []
  const chat = Array.isArray(listing?.chat) ? listing.chat : []
  const out = [...app]
  let after = -1
  for (const m of chat) {
    const i = out.findIndex((x) => x?.key === m?.key)
    if (i >= 0) {
      after = i
      continue
    }
    out.splice(++after, 0, m)
  }
  return out
}

// modelInfos is the listing's enabled models an agent can pick: "auto"
// and "default" route inside Qoder and aren't one model.
function modelInfos(listing) {
  return sceneModels(listing)
    .filter((m) => m?.enable && !["", "auto", "default"].includes(String(m.key ?? "").trim()))
    .map(modelInfo)
}


// Frozen rev3 source lines 631-1291.
const EFFORT_RANK = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]

function fitEffort(want, levels) {
  if (want === "ultra" && !levels.includes(want)) want = "max"
  if (!levels.length || levels.includes(want)) return want
  const at = EFFORT_RANK.indexOf(want)
  if (at < 0) return want
  let best = want
  let dist = EFFORT_RANK.length
  for (const l of levels) {
    const i = EFFORT_RANK.indexOf(l)
    if (i < 0 || l === "none") continue
    const d = Math.abs(i - at)
    if (d < dist || (d === dist && i > at)) [best, dist] = [l, d]
  }
  return best
}

// effortFor is whether to think and at which of the model's efforts ("" for
// none): asked for none, a model that always thinks thinks at its lowest;
// asked for nothing, or a level Qoder doesn't name, at its own default.
function effortFor(asked, m) {
  let want = String(asked ?? "").toLowerCase()
  if (!EFFORT_RANK.includes(want)) want = ""
  if (!m.thinks || (want === "none" && !m.alwaysThinks)) return [false, ""]
  if (!m.efforts.length) return [true, ""]
  if (want === "") return [true, m.defaultEffort]
  if (want === "none") return [true, m.efforts[0]]
  return [true, fitEffort(want, m.efforts)]
}

function imageBlock(p) {
  const url = typeof p.image_url === "string" ? p.image_url : p.image_url?.url
  return url ? { type: "image_url", image_url: { url } } : null
}

function blocksOf(content) {
  if (typeof content === "string") return content ? [{ type: "text", text: content }] : []
  const out = []
  for (const p of content ?? []) {
    if (p?.type === "text" && p.text != null) out.push({ type: "text", text: p.text })
    else if (p?.type === "image_url") {
      const b = imageBlock(p)
      if (b) out.push(b)
    }
  }
  return out
}

const textOf = (c) => (typeof c === "string" ? c : blocksOf(c).filter((b) => b.type === "text").map((b) => b.text).join(""))

// qoderMessages is the chat's turns as Qoder takes them: text and image
// blocks, an assistant's tool calls as tool_calls, results as tool turns by
// tool_call_id. A tool turn holds text only, so the images a tool returned
// follow in a user turn — the start of the user's own when one comes next.
function qoderMessages(msgs) {
  const out = []
  const names = {}
  let seen = []
  const showSeen = () => {
    if (seen.length) out.push({ role: "user", content: seen })
    seen = []
  }
  for (const m of msgs) {
    if (m.role === "system" || m.role === "developer") continue
    if (m.role !== "user" && m.role !== "tool") showSeen()
    if (m.role === "tool") {
      let txt = textOf(m.content)
      const ims = Array.isArray(m.content) ? m.content.filter((p) => p?.type === "image_url").map(imageBlock).filter(Boolean) : []
      if (ims.length) {
        let of = "tool call " + m.tool_call_id
        const name = names[m.tool_call_id] || m.name
        if (name) of = `${name} (${of})`
        seen.push({ type: "text", text: `[From the result of ${of}:]` }, ...ims)
        const note = ims.length === 1 ? "[The tool returned an image; it follows in the next message.]" : `[The tool returned ${ims.length} images; they follow in the next message.]`
        txt = txt.trim() ? txt + "\n\n" + note : txt + note
      }
      out.push({ role: "tool", tool_call_id: m.tool_call_id, content: txt })
      continue
    }
    if (m.role === "assistant" && m.tool_calls?.length) {
      const calls = m.tool_calls.map((c) => {
        const id = c.id || "call_" + hexID()
        names[id] = c.function?.name
        const args = c.function?.arguments
        return { id, type: "function", function: { name: c.function?.name, arguments: typeof args === "string" ? args : JSON.stringify(args ?? {}) } }
      })
      out.push({ role: "assistant", content: textOf(m.content), tool_calls: calls })
      continue
    }
    let blocks = blocksOf(m.content)
    if (m.role === "assistant") blocks = blocks.filter((b) => b.type === "text")
    if (!blocks.length) continue
    if (m.role === "user" && seen.length) [blocks, seen] = [[...seen, ...blocks], []]
    out.push({ role: m.role, content: blocks })
  }
  showSeen()
  return out
}

// windowFor is the context window a request is sent (context_length) on a
// model that offers several: the one Qoder's client picks by default
// (max_input_tokens without one) while the request fits it, else the
// smallest that holds it, else the largest. A larger window may cost more
// (Qoder: changing the Context "may change the rate"), so it is asked for
// only when the conversation needs it. The request's size is counted
// generously, a token for every 3 bytes of its UTF-8 JSON, so a window is
// raised a little early rather than late, and the reply's max_tokens is
// counted in it: a window holds the prompt and the reply, so a
// conversation of 180K asking 32K for the reply is sent 400K, not the 200K
// it would overflow while magpie says the model takes 1M (yetone/magpie#700).
function windowFor(chat, m, reply = 0) {
  const base = m.defaultWindow || (m.config?.max_input_tokens > 0 ? m.config.max_input_tokens : 0) || m.context
  if (!m.windows?.length) return m.context
  const need = Math.ceil(Buffer.byteLength(JSON.stringify({ m: chat.messages ?? [], t: chat.tools ?? [] })) / 3) + Math.max(0, reply)
  if (base > 0 && need <= base) return base
  return m.windows.find((w) => w >= need) ?? m.windows[m.windows.length - 1]
}

// qoderBody is the plaintext agent_chat_generation takes for a chat
// completion request, for model m.
function qoderBody(chat, m, region = "international") {
  const system = (chat.messages ?? []).filter((x) => x.role === "system" || x.role === "developer").map((x) => textOf(x.content)).filter(Boolean).join("\n\n")
  const tools = chat.tool_choice !== "none" ? (chat.tools ?? []).filter((t) => t?.type === "function" && t.function?.name) : []
  let sysText = system ? QODER_SYS + "\n\n" + system : QODER_SYS
  if (chat.tool_choice === "required" && tools.length) sysText += "\nYou must call an available function in this response."
  const sys = { type: "text", text: sysText }
  const [thinking, effort] = effortFor(chat.reasoning_effort, m)
  const params = { enable_thinking: thinking, max_tokens: chat.max_completion_tokens || chat.max_tokens || 32000 }
  if (effort) params.reasoning_effort = effort
  const window = windowFor(chat, m, params.max_tokens)
  if (window > 0) params.context_length = window
  const body = {
    parameters: params,
    business: { product: "app", version: COSY_VERSION, type: "agent", id: hexID(), name: "magpie session", begin_at: Date.now(), stage: "start" },
    agent_id: "agent_common",
    task_id: "common",
    session_type: "app",
    model_config: m.config,
    system: [sys],
    messages: [{ role: "system", content: [sys] }, ...qoderMessages(chat.messages ?? [])],
  }
  if (region === "cn") {
    // CN CLI wire fields; independently mapped onto the existing request serializer.
    // Protocol facts: dsh-provider-qoder src/adapter.ts (MIT), no credential/cache reuse.
    const lastUser = [...(chat.messages ?? [])].reverse().find(x => x.role === "user");
    const prompt = textOf(lastUser?.content);
    Object.assign(body, {
      request_id: randomUUID(), request_set_id: randomUUID(), chat_record_id: randomUUID(), session_id: randomUUID(),
      stream: true, chat_task: "FREE_INPUT", is_reply: true, is_retry: false, source: 1, version: "3",
      session_type: "qodercli", code_language: "", chat_prompt: "", image_urls: null, aliyun_user_type: "",
      business: {...body.business, product: "cli", version: "1.0.0"},
      chat_context: {chatPrompt: "", imageUrls: null, extra: {context: [], modelConfig: m.config, originalContent: prompt}, features: [], text: prompt},
    });
  }
  if (tools.length)
    body.tools = tools.map((t) => ({
      type: "function",
      function: { name: t.function.name, description: t.function.description ?? "", parameters: t.function.parameters ?? { type: "object", properties: {} } },
    }))
  return body
}

// failure is the status and message for a Qoder failure. Qoder's own code
// says what it is, read as Qoder's client reads it (QoderErrorCode in the
// app's qoder-agent-sdk worker): only 105 (loginExpired) is a lapsed
// sign-in there, so a 401/403 that carries another code isn't one here
// either (#98: a queued 10605 came as a 403 and read as "sign in again").
// A refused chat with no code keeps the built-in's 401, and a quota is a
// 429.
const QUEUED = "10605"
// the codes Qoder's client counts as a plan's usage or credits used up
const QUOTA_CODES = new Set(["110", "112", "113", "114", "115", "116", "117", "118", "119", "122"])
// refusals of this request, not of the account: sensitive content, not
// supported, input too long, too many media, a custom model's service
const REQUEST_CODES = new Set(["406", "416", "430", "80411", "80412", "100400", "100401", "100403"])
const BUSY_WAIT = 30

function classifyFailure(status, text) {
  if (status < 400 || status > 599) status = 502
  let msg = String(text ?? "").trim()
  let code = ""
  let queue
  try {
    let j = JSON.parse(msg)
    // #167: an HTTP/message wrapper can hold the actual 10605 body.
    // Unwrap only that known queue code; a real auth/credit/request code
    // on the outer body remains authoritative.
    const outerCode = j?.code == null ? "" : String(j.code)
    if (["", "401", "403"].includes(outerCode) && typeof j?.message === "string") {
      try {
        const inner = JSON.parse(j.message.trim())
        if (inner && !Array.isArray(inner) && String(inner.code) === QUEUED) j = inner
      } catch {}
    }
    code = j?.code == null ? "" : String(j.code)
    if (j?.message) msg = String(j.message)
    // details is a JSON string, or the object itself (gjson reads either)
    const d = typeof j?.details === "string" ? JSON.parse(j.details.trim()) : j?.details
    if (d?.error?.message) msg += ": " + d.error.message
    // the queue's state: the message itself, as a JSON string, or a field
    queue = j?.queue && typeof j.queue === "object" ? j.queue : undefined
    if (!queue && typeof j?.message === "string" && j.message.trim().startsWith("{")) {
      const q = JSON.parse(j.message.trim())
      if (q && typeof q === "object" && ("isQueued" in q || "serviceAvailable" in q || "retryAfterSeconds" in q)) queue = q
    }
  } catch {}
  msg ||= STATUS_CODES[status] ?? `HTTP ${status}`
  // Qoder's queue for the model: busy for now, with when to ask again. The
  // sign-in is fine (the same token reads the models and the usage), so it
  // is a 503 with Retry-After, which magpie asks again after, and never
  // touches the account's sign-in.
  if (code === QUEUED || queue?.isQueued === true || queue?.serviceAvailable === false) {
    const n = Number(queue?.retryAfterSeconds)
    const wait = Number.isFinite(n) && n > 0 ? Math.ceil(n) : BUSY_WAIT
    const model = queue?.modelKey ? ` for ${queue.modelKey}` : ""
    const tier = queue?.queueType ? ` (this plan's queue: ${queue.queueType})` : ""
    const what = queue?.isQueued === true ? "queued the request" : "is busy"
    return { status: 503, message: `Qoder ${what}${model}${tier}; try again in ${wait}s`, retryAfter: wait, signIn: "" }
  }
  if (QUOTA_CODES.has(code)) return { status: 429, message: "usage limit reached: " + msg, signIn: "" }
  if (REQUEST_CODES.has(code)) return { status: 400, message: `Qoder refused the request (${code}): ${msg}`, signIn: "" }
  // a refused chat is the built-in's 401, which never marked the account
  // lapsed: only a refused refresh did (errorResponse says kept). Qoder's
  // own words go with it: a 401 right after signing in isn't a lapsed
  // sign-in, and its reason is what says what is (Yayoi_no_yume on X).
  if (status === 401 || status === 403) {
    const said = msg && msg !== STATUS_CODES[status] ? ` (Qoder said ${status}: ${msg.slice(0, 300)})` : ""
    return { status: 401, message: "the sign-in lapsed — sign in again" + said }
  }
  if (status === 429 || msg.toLowerCase().includes("quota")) return { status: 429, message: "usage limit reached: " + msg }
  return { status, message: msg }
}

// errorResponse is a failure as OpenAI's API gives it. signIn is what it
// means for the account (magpie's X-Magpie-Sign-In): a 401 keeps it unless
// told expired, since the built-in marked only a refused refresh. retryAfter
// (seconds) is sent as Retry-After.
const errorResponse = ({ status, message, signIn = status === 401 ? "kept" : undefined, retryAfter }) =>
  new Response(JSON.stringify({ error: { message, type: "qoder_error", code: status } }), {
    status,
    headers: {
      "Content-Type": "application/json",
      ...(signIn ? { "X-Magpie-Sign-In": signIn } : {}),
      ...(retryAfter > 0 ? { "Retry-After": String(retryAfter) } : {}),
    },
  })

// ---- the reply ----------------------------------------------------------------

const CALL_OPEN = "\x3ctool_call\x3e"
const CALL_CLOSE = "\x3c/tool_call\x3e"
const FUNC = /\x3cfunction=([^>]+)\x3e([\s\S]*?)\x3c\/function\x3e/
const PARAM = /\x3cparameter=([^>]+)\x3e([\s\S]*?)\x3c\/parameter\x3e/g

// parseCall reads one tool-call block: JSON {name, arguments}, or the XML
// function/parameter form.
function parseCall(v) {
  try {
    const f = JSON.parse(v.trim())
    if (String(f?.name ?? "").trim() && f.arguments && typeof f.arguments === "object" && !Array.isArray(f.arguments))
      return { name: f.name.trim(), args: JSON.stringify(f.arguments) }
  } catch {}
  const m = FUNC.exec(v)
  if (!m || !m[1].trim()) return null
  const args = {}
  for (const pm of m[2].matchAll(PARAM)) {
    const key = pm[1].trim()
    if (!key) continue
    const val = pm[2].trim()
    try {
      args[key] = JSON.parse(val)
    } catch {
      args[key] = val
    }
  }
  return { name: m[1].trim(), args: JSON.stringify(args) }
}

// quoted says text ends inside a code fence or an inline code span: a
// <tool_call> there is prose about one, not a call
function quoted(text) {
  if ((text.match(/```/g)?.length ?? 0) % 2) return true
  const line = text.slice(text.lastIndexOf("\n") + 1).replace(/```/g, "")
  return (line.match(/`/g)?.length ?? 0) % 2 === 1
}

// callish says a block was on its way to being a call: its JSON began, or
// the XML function form
const callish = (v) => v.trimStart().startsWith("{") || v.includes("\x3cfunction=")

const CUT_SHORT = "the model's tool call, written as text, was cut short; it is not sent on as the answer"
const UNREADABLE = "the model's tool call, written as text, can't be read; it is not sent on as the answer"

// Splitter holds text until a whole tool-call block has come, then gives
// it as a call. A block on its way to being a call that can't be read, or
// that the stream ended inside, is an error, never the answer (plugins#132,
// as trae's since plugins#38): the turn fails so it can be tried again. A
// block whose JSON is whole when the stream ends without its close is still
// the call; one that is no call at all, or one in code, stays text.
class Splitter {
  textBuf = ""
  callBuf = ""
  inCall = false
  sawTool = false
  sent = "" // the text given out so far, to tell a block in code from a call
  text(out, t) {
    this.sent += t
    out.push({ text: t })
  }
  feed(input) {
    const out = []
    while (input) {
      if (!this.inCall) {
        const comb = this.textBuf + input
        this.textBuf = ""
        const i = comb.indexOf(CALL_OPEN)
        if (i < 0) {
          let keep = 0
          for (let n = CALL_OPEN.length - 1; n > 0; n--)
            if (comb.endsWith(CALL_OPEN.slice(0, n))) {
              keep = n
              break
            }
          if (keep < comb.length) this.text(out, comb.slice(0, comb.length - keep))
          this.textBuf = comb.slice(comb.length - keep)
          break
        }
        if (i > 0) this.text(out, comb.slice(0, i))
        input = comb.slice(i + CALL_OPEN.length)
        this.inCall = true
        continue
      }
      const comb = this.callBuf + input
      this.callBuf = ""
      const j = comb.indexOf(CALL_CLOSE)
      if (j < 0) {
        this.callBuf = comb
        break
      }
      const body = comb.slice(0, j)
      const c = parseCall(body)
      if (c) {
        this.sawTool = true
        out.push({ call: c })
      } else if (callish(body) && !quoted(this.sent)) {
        this.inCall = false
        out.push({ error: UNREADABLE })
        return out
      } else this.text(out, CALL_OPEN + body + CALL_CLOSE)
      input = comb.slice(j + CALL_CLOSE.length)
      this.inCall = false
    }
    return out
  }
  flush() {
    if (this.inCall) {
      const body = this.callBuf
      this.inCall = false
      this.callBuf = ""
      if (callish(body) && !quoted(this.sent)) {
        const c = parseCall(body)
        if (!c) return [{ error: CUT_SHORT }]
        this.sawTool = true
        return [{ call: c }]
      }
      const t = CALL_OPEN + body
      this.sent += t
      return [{ text: t }]
    }
    if (this.textBuf) {
      const t = this.textBuf
      this.textBuf = ""
      this.sent += t
      return [{ text: t }]
    }
    return []
  }
}

async function* sseLines(body) {
  const dec = new TextDecoder()
  let buf = ""
  for await (const chunk of body) {
    buf += dec.decode(chunk, { stream: true })
    let n
    while ((n = buf.indexOf("\n")) >= 0) {
      yield buf.slice(0, n).trim()
      buf = buf.slice(n + 1)
    }
  }
  if (buf.trim()) yield buf.trim()
}

// Local protocol-validation candidate. The original Splitter is reused unchanged.
// Tools are buffered until a terminal and validation; ordinary text still streams.
const TOOL_LIMIT = 64
const TOOL_ARGS_LIMIT = 256 * 1024
const TOOL_TOTAL_LIMIT = 1024 * 1024
const protocolError = (message, status = 502) => ({ error: { status, message: `Qoder protocol: ${message}` } })

function toolPolicy(chat) {
  const choice = chat.tool_choice === undefined ? "auto" : chat.tool_choice
  if (chat.tools != null && !Array.isArray(chat.tools)) return { error: "tools must be an array" }
  const names = new Set((chat.tools ?? []).filter((t) => t?.type === "function" && typeof t.function?.name === "string" && t.function.name).map((t) => t.function.name))
  let mode = choice
  let named = ""
  if (choice && typeof choice === "object" && !Array.isArray(choice) && choice.type === "function" && typeof choice.function?.name === "string" && choice.function.name) {
    mode = "named"
    named = choice.function.name
    if (!names.has(named)) return { error: "named tool_choice is not an available function" }
  } else if (!["auto", "none", "required"].includes(choice)) return { error: "unsupported tool_choice" }
  if (chat.parallel_tool_calls != null && typeof chat.parallel_tool_calls !== "boolean") return { error: "parallel_tool_calls must be a boolean" }
  return { names, mode, named, parallel: chat.parallel_tool_calls !== false }
}

function validTools(calls, policy) {
  if (policy.mode === "none" && calls.length) return "tool_choice none forbids tool calls"
  if ((policy.mode === "required" || policy.mode === "named") && !calls.length) return "tool_choice requires a tool call"
  if (!policy.parallel && calls.length > 1) return "parallel_tool_calls false forbids multiple calls"
  const ids = new Set()
  for (const call of calls) {
    if (!call.name || !policy.names.has(call.name)) return "returned tool is not an available function"
    if (policy.mode === "named" && call.name !== policy.named) return "returned tool does not match named tool_choice"
    if (!call.id || ids.has(call.id)) return "missing or duplicate tool call id"
    ids.add(call.id)
    let args
    try { args = JSON.parse(call.args) } catch { return "tool arguments are not complete JSON" }
    if (!args || typeof args !== "object" || Array.isArray(args)) return "tool arguments must be a JSON object"
  }
  // Deliberately no arbitrary JSON Schema validator. Outbound schemas stay intact.
  return ""
}

// events reads the original Qoder envelope. Native fragments are keyed by their
// original index; text-form calls continue to use the upstream Splitter.
async function* events(body, chat = {}) {
  const policy = toolPolicy(chat)
  if (policy.error) { yield protocolError(policy.error, 400); return }
  const a = new Splitter()
  const calls = []
  const native = new Map()
  let argsBytes = 0
  let usable = false
  let terminal = false
  let fr = ""
  let usage
  const bufferTextCall = (f) => {
    if (f.error) return f.error
    if (!f.call) return ""
    if (calls.length >= TOOL_LIMIT) return "tool call count exceeds the limit"
    const bytes = Buffer.byteLength(f.call.args)
    argsBytes += bytes
    if (bytes > TOOL_ARGS_LIMIT || argsBytes > TOOL_TOTAL_LIMIT) return "tool argument buffer exceeds the limit"
    calls.push({ index: calls.length, id: "call_" + hexID(), name: f.call.name, args: f.call.args })
    usable = true
    return ""
  }
  try {
    for await (const line of sseLines(body)) {
      if (!line.startsWith("data:")) continue
      const payload = line.slice(5).trim()
      let env
      try { env = JSON.parse(payload) } catch { yield protocolError("invalid SSE envelope JSON"); return }
      if (!env || typeof env !== "object" || Array.isArray(env) || typeof env.body !== "string") { yield protocolError("invalid SSE envelope"); return }
      const inner = env.body.trim()
      if (env.statusCodeValue != null && env.statusCodeValue !== 200) {
        yield { error: failure(Number(env.statusCodeValue), inner || payload) }
        return
      }
      if (inner === "[DONE]") { terminal = true; break }
      let chunk
      try { chunk = JSON.parse(inner) } catch { yield protocolError("invalid SSE body JSON"); return }
      if (!chunk || typeof chunk !== "object" || Array.isArray(chunk) || (!Array.isArray(chunk.choices) && !chunk.usage)) { yield protocolError("invalid completion chunk"); return }
      if ((chunk.choices?.length ?? 0) > 1) { yield protocolError("multiple completion choices are unsupported"); return }
      const choice = chunk.choices?.[0]
      if (choice && Object.hasOwn(choice, "index") && choice.index !== 0) { yield protocolError("unsupported completion choice index"); return }
      const delta = choice?.delta ?? {}
      if (!delta || typeof delta !== "object" || Array.isArray(delta)) { yield protocolError("invalid completion delta"); return }
      if ((delta.content != null && typeof delta.content !== "string") || (delta.reasoning_content != null && typeof delta.reasoning_content !== "string") || (delta.tool_calls != null && !Array.isArray(delta.tool_calls))) { yield protocolError("invalid completion delta fields"); return }
      if (terminal && (delta.content || delta.reasoning_content || delta.tool_calls?.length)) { yield protocolError("content arrived after finish_reason"); return }
      if (delta.reasoning_content) { usable = true; yield { reasoning: delta.reasoning_content } }
      if (typeof delta.content === "string") {
        for (const f of a.feed(delta.content)) {
          const err = bufferTextCall(f)
          if (err) { yield protocolError(err); return }
          if (f.text) { usable = true; yield { text: f.text } }
        }
        if (Buffer.byteLength(a.callBuf) > TOOL_ARGS_LIMIT) { yield protocolError("text tool buffer exceeds the limit"); return }
      }
      for (const tc of delta.tool_calls ?? []) {
        if (tc && Object.hasOwn(tc, "type") && tc.type !== "function") { yield protocolError("unsupported native tool type"); return }
        const i = tc?.index ?? 0
        if (!Number.isSafeInteger(i) || i < 0) { yield protocolError("invalid native tool index"); return }
        let call = native.get(i)
        if (!call) {
          for (const f of a.flush()) {
            const err = bufferTextCall(f)
            if (err) { yield protocolError(err); return }
            if (f.text) { usable = true; yield { text: f.text } }
          }
          if (calls.length >= TOOL_LIMIT) { yield protocolError("tool call count exceeds the limit"); return }
          call = { index: calls.length, id: "", name: "", args: "" }
          native.set(i, call)
          calls.push(call)
        }
        if (tc.id != null) {
          if (typeof tc.id !== "string" || !tc.id || (call.id && call.id !== tc.id)) { yield protocolError("conflicting or invalid native tool id"); return }
          call.id = tc.id
        }
        if (tc.function?.name != null) {
          if (typeof tc.function.name !== "string" || !tc.function.name || (call.name && call.name !== tc.function.name)) { yield protocolError("conflicting or invalid native tool name"); return }
          call.name = tc.function.name
        }
        if (tc.function?.arguments != null) {
          if (typeof tc.function.arguments !== "string") { yield protocolError("native tool arguments must be string fragments"); return }
          argsBytes += Buffer.byteLength(tc.function.arguments)
          call.args += tc.function.arguments
          if (Buffer.byteLength(call.args) > TOOL_ARGS_LIMIT || argsBytes > TOOL_TOTAL_LIMIT) { yield protocolError("tool argument buffer exceeds the limit"); return }
        }
        usable = true
      }
      if (choice?.finish_reason != null) {
        if (!["stop", "length", "tool_calls"].includes(choice.finish_reason)) { yield protocolError("unsupported finish_reason"); return }
        if (fr && fr !== choice.finish_reason) { yield protocolError("conflicting finish_reason"); return }
        fr = choice.finish_reason
        terminal = true
      }
      // Preserve usage-only tail chunks after finish_reason and before [DONE].
      const u = chunk.usage
      if (u) usage = {
        prompt_tokens: u.prompt_tokens ?? 0,
        completion_tokens: u.completion_tokens ?? 0,
        total_tokens: (u.prompt_tokens ?? 0) + (u.completion_tokens ?? 0),
        ...(u.prompt_tokens_details?.cached_tokens ? { prompt_tokens_details: { cached_tokens: u.prompt_tokens_details.cached_tokens } } : {}),
        ...(u.completion_tokens_details?.reasoning_tokens ? { completion_tokens_details: { reasoning_tokens: u.completion_tokens_details.reasoning_tokens } } : {}),
      }
    }
  } catch {
    yield protocolError("response stream failed")
    return
  }
  for (const f of a.flush()) {
    const err = bufferTextCall(f)
    if (err) { yield protocolError(err); return }
    if (f.text) { usable = true; yield { text: f.text } }
  }
  if (!terminal) { yield protocolError("response ended without a valid terminal"); return }
  if (!usable) { yield protocolError("response ended without an answer"); return }
  if (fr === "tool_calls" && !calls.length) { yield protocolError("tool_calls finish without a call"); return }
  if (fr === "length" && calls.length) { yield protocolError("tool response ended at the token limit"); return }
  const err = validTools(calls, policy)
  if (err) { yield protocolError(err); return }
  for (const call of calls) yield { tool: call }
  yield { stop: calls.length ? "tool_calls" : fr === "length" ? "length" : "stop", usage }
}

// answer turns Qoder's reply into the chat completion OpenAI's API gives.
// A failure before the answer keeps its status; one after it ends the
// stream with an error.
async function answer(res, chat, { abortUpstream = () => {}, releaseListener = () => {} } = {}) {
  const id = "chatcmpl-" + hexID()
  const created = Math.floor(Date.now() / 1000)
  const it = events(res.body, chat)
  let handedToStream = false
  let released = false
  const release = () => {
    if (released) return
    released = true
    releaseListener()
  }
  const retire = async () => {
    try { await it.return?.() } catch {} finally { release() }
  }
  try {
    const first = await it.next()
    if (first.value?.error) {
      abortUpstream(first.value.error)
      return errorResponse(first.value.error)
    }
    if (first.done) {
      abortUpstream()
      return errorResponse({ status: 502, message: "Qoder ended without an answer" })
    }

    if (!chat.stream) {
      const msg = { role: "assistant", content: "" }
      let reasoning = ""
      const calls = []
      let stop = "stop"
      let usage
      for (let r = first; !r.done; r = await it.next()) {
        const e = r.value
        if (e.error) {
          abortUpstream(e.error)
          return errorResponse(e.error)
        }
        if (e.text) msg.content += e.text
        if (e.reasoning) reasoning += e.reasoning
        if (e.tool) calls.push({ id: e.tool.id, type: "function", function: { name: e.tool.name, arguments: e.tool.args } })
        if (e.args) calls[calls.length - 1].function.arguments += e.args.text
        if (e.stop) [stop, usage] = [e.stop, e.usage]
      }
      if (reasoning) msg.reasoning_content = reasoning
      if (calls.length) msg.tool_calls = calls
      return Response.json({ id, object: "chat.completion", created, model: chat.model, choices: [{ index: 0, message: msg, finish_reason: stop }], ...(usage ? { usage } : {}) })
    }

    const enc = new TextEncoder()
    const chunk = (delta, finish_reason = null, extra = {}) => {
      const bytes = enc.encode(`data: ${JSON.stringify({ id, object: "chat.completion.chunk", created, model: chat.model, choices: [{ index: 0, delta, finish_reason }], ...extra })}\n\n`)
      // The private worker splits ordinary content at 48 KiB. Keep the final
      // finish frame indivisible so the host can withhold success until reap.
      if (finish_reason !== null && bytes.byteLength > 48 * 1024) throw new Error("QODER_TERMINAL_TOO_LARGE")
      return bytes
    }
    let pending = first
    let began = false
    let closed = false
    let cancelled = false
    const stream = new ReadableStream({
      async pull(ctl) {
        if (closed) return
        try {
          const r = pending ?? (await it.next())
          pending = null
          // A concurrent cancel aborts the pending network read before it settles.
          if (closed) return
          if (!began) {
            began = true
            ctl.enqueue(chunk({ role: "assistant", content: "" }))
          }
          if (r.done) {
            closed = true
            release()
            ctl.enqueue(enc.encode("data: [DONE]\n\n"))
            return ctl.close()
          }
          const e = r.value
          if (e.text) ctl.enqueue(chunk({ content: e.text }))
          else if (e.reasoning) ctl.enqueue(chunk({ reasoning_content: e.reasoning }))
          else if (e.tool) ctl.enqueue(chunk({ tool_calls: [{ index: e.tool.index, id: e.tool.id, type: "function", function: { name: e.tool.name, arguments: e.tool.args } }] }))
          else if (e.args) ctl.enqueue(chunk({ tool_calls: [{ index: e.args.index, function: { arguments: e.args.text } }] }))
          else if (e.stop) ctl.enqueue(chunk({}, e.stop, e.usage ? { usage: e.usage } : {}))
          else if (e.error) {
            closed = true
            abortUpstream(e.error)
            await retire()
            if (cancelled) return
            ctl.enqueue(enc.encode(`data: ${JSON.stringify({ error: { message: e.error.message, code: e.error.status } })}\n\n`))
            ctl.close()
          }
        } catch (e) {
          if (!closed) {
            closed = true
            abortUpstream(e)
          }
          await retire()
          if (!cancelled) ctl.error(e)
        }
      },
      async cancel(reason) {
        cancelled = true
        closed = true
        // Synchronous abort first: iterator.return alone cannot interrupt next().
        abortUpstream(reason)
        await retire()
      },
    })
    const response = new Response(stream, { status: 200, headers: { "Content-Type": "text/event-stream", "Cache-Control": "no-cache" } })
    handedToStream = true
    return response
  } catch (e) {
    abortUpstream(e)
    throw e
  } finally {
    if (!handedToStream) await retire()
  }
}


function failure(status, text) { const result = classifyFailure(status, text); return {status: result.status, message: "QODER_UPSTREAM_REJECTED"}; }

// New local admission/transport boundary. No auth lifecycle or endpoint override.
const GATEWAYS = Object.freeze({international:'https://api3.qoder.sh',cn:'https://gateway.qoder.com.cn'});
export const MAX_RESPONSE_BODY_BYTES = 128 * 1024 * 1024;
const MAX_INPUT_BYTES = 1024 * 1024;
const MAX_MODEL_BYTES = 1024 * 1024;
const REQUEST_DEADLINE_MS = 120000;
const object = v => v !== null && typeof v === 'object' && !Array.isArray(v);
const boundedString = (v, n = 4096) => typeof v === 'string' && v.length > 0 && v.length <= n && !/[\x00-\x1f\x7f]/.test(v);
const only = (v, keys) => object(v) && Object.keys(v).every(k => keys.includes(k));
const fixedResponse = (status, code) => Response.json({error:{type:'qoder_error',code,message:code,retryable:false}},{status,headers:{'cache-control':'no-store'}});
const fixedError = code => Object.assign(new Error(code),{code});

export function admit(snapshot, chat) {
  if (!only(snapshot,['access','uid','machineId','expiry','region']) || !Object.hasOwn(GATEWAYS,snapshot.region)) return 'QODER_INVALID_SNAPSHOT';
  if (!boundedString(snapshot.access,16384) || !boundedString(snapshot.uid,1024) || !boundedString(snapshot.machineId,1024)) return 'QODER_INVALID_SNAPSHOT';
  if (!Number.isSafeInteger(snapshot.expiry) || snapshot.expiry <= Date.now()) return 'QODER_SNAPSHOT_EXPIRED';
  if (!only(chat,['model','messages','tools','tool_choice','parallel_tool_calls','stream','max_tokens','max_completion_tokens','reasoning_effort']) || !boundedString(chat.model,256)) return 'QODER_INVALID_REQUEST';
  if (!Array.isArray(chat.messages) || !chat.messages.length || chat.messages.length>4096 || (chat.stream!==undefined && typeof chat.stream!=='boolean')) return 'QODER_INVALID_REQUEST';
  for (const key of ['max_tokens','max_completion_tokens']) if (chat[key]!==undefined && (!Number.isSafeInteger(chat[key]) || chat[key]<1)) return 'QODER_INVALID_REQUEST';
  if (chat.reasoning_effort!==undefined && !EFFORT_RANK.includes(chat.reasoning_effort)) return 'QODER_INVALID_REQUEST';
  if (chat.tools!==undefined && (!Array.isArray(chat.tools) || chat.tools.length>TOOL_LIMIT)) return 'QODER_INVALID_TOOLS';
  const names = new Set();
  for (const tool of chat.tools ?? []) {
    if (!only(tool,['type','function']) || tool.type!=='function' || !only(tool.function,['name','description','parameters','strict']) || !boundedString(tool.function.name,256) || names.has(tool.function.name)) return 'QODER_INVALID_TOOLS';
    if (tool.function.description!==undefined && typeof tool.function.description!=='string') return 'QODER_INVALID_TOOLS';
    if (tool.function.strict!==undefined && tool.function.strict!==false) return 'QODER_UNSUPPORTED_REQUEST';
    if (tool.function.parameters!==undefined && !object(tool.function.parameters) && typeof tool.function.parameters!=='boolean') return 'QODER_INVALID_TOOLS';
    names.add(tool.function.name);
  }
  const policy=toolPolicy(chat);
  if (policy.error || ((policy.mode==='required' || policy.mode==='named') && !names.size)) return 'QODER_INVALID_TOOL_POLICY';
  if (object(chat.tool_choice) && (!only(chat.tool_choice,['type','function']) || !only(chat.tool_choice.function,['name']))) return 'QODER_INVALID_TOOL_POLICY';
  const ids=new Set(), pending=new Set();
  for (const m of chat.messages) {
    if (!only(m,['role','content','tool_calls','tool_call_id','name']) || !['system','developer','user','assistant','tool'].includes(m.role)) return 'QODER_INVALID_HISTORY';
    const content=m.content;
    if (content!==null && content!==undefined && typeof content!=='string') {
      if (!Array.isArray(content)) return 'QODER_INVALID_HISTORY';
      for (const part of content) {
        if (part?.type==='text') { if (!only(part,['type','text']) || typeof part.text!=='string') return 'QODER_INVALID_HISTORY'; }
        else if (part?.type==='image_url' && ['user','tool'].includes(m.role)) {
          if (!only(part,['type','image_url'])) return 'QODER_INVALID_HISTORY';
          const image=part.image_url;
          if (typeof image==='string') {if (!image) return 'QODER_INVALID_HISTORY';}
          else if (!only(image,['url']) || typeof image.url!=='string' || !image.url) return 'QODER_INVALID_HISTORY';
        } else return 'QODER_INVALID_HISTORY';
      }
    }
    if (m.name!==undefined && !boundedString(m.name,256)) return 'QODER_INVALID_HISTORY';
    if (m.role==='tool') {
      if (!boundedString(m.tool_call_id,256) || !pending.delete(m.tool_call_id) || m.tool_calls!==undefined) return 'QODER_INVALID_HISTORY';
    } else {
      if (pending.size || m.tool_call_id!==undefined) return 'QODER_INVALID_HISTORY';
      if (m.tool_calls!==undefined) {
        if (m.role!=='assistant' || !Array.isArray(m.tool_calls) || !m.tool_calls.length || m.tool_calls.length>TOOL_LIMIT) return 'QODER_INVALID_HISTORY';
        for (const c of m.tool_calls) {
          if (!only(c,['id','type','function']) || c.type!=='function' || !boundedString(c.id,256) || ids.has(c.id) || !only(c.function,['name','arguments']) || !boundedString(c.function.name,256) || typeof c.function.arguments!=='string') return 'QODER_INVALID_HISTORY';
          let args;try {args=JSON.parse(c.function.arguments);} catch {return 'QODER_INVALID_HISTORY';}
          if (!object(args)) return 'QODER_INVALID_HISTORY';
          ids.add(c.id);pending.add(c.id);
        }
      }
    }
  }
  if (pending.size) return 'QODER_INVALID_HISTORY';
  if (Buffer.byteLength(JSON.stringify({snapshot,chat}))>=MAX_INPUT_BYTES) return 'QODER_INPUT_TOO_LARGE';
  return '';
}

// Bounds upstream bytes before the original SSE parser buffers any text.
function boundedBody(body, limit, abort, onFailure = () => {}) {
  const reader=body.getReader();let total=0,closed=false;
  return new ReadableStream({
    async pull(ctl){if(closed)return;try{const r=await reader.read();if(closed)return;if(r.done){closed=true;reader.releaseLock();ctl.close();return;}total+=r.value.byteLength;if(total>limit){closed=true;onFailure();abort();await reader.cancel().catch(()=>{});reader.releaseLock();ctl.error(fixedError('QODER_RESPONSE_TOO_LARGE'));return;}ctl.enqueue(r.value);}catch{if(closed)return;closed=true;onFailure();abort();try{reader.releaseLock();}catch{}ctl.error(fixedError('QODER_TRANSPORT_FAILED'));}},
    async cancel(){if(closed)return;closed=true;abort();try{await reader.cancel();}catch{}finally{try{reader.releaseLock();}catch{}}},
  });
}

export async function complete(snapshot, chat, {signal}={}) {
  const invalid=admit(snapshot,chat);
  if(invalid)return fixedResponse(invalid==='QODER_SNAPSHOT_EXPIRED'?401:400,invalid);
  const controller=new AbortController();let linked=false,handed=false,submitted=false,transportBroken=false;
  const abort=()=>{if(!controller.signal.aborted)controller.abort(fixedError('QODER_REQUEST_ABORTED'));};
  const release=()=>{clearTimeout(timer);if(linked){linked=false;signal.removeEventListener('abort',abort);}};
  const timer=setTimeout(abort,REQUEST_DEADLINE_MS);
  if(signal?.aborted)abort();else if(signal){signal.addEventListener('abort',abort,{once:true});linked=true;}
  try {
    if(controller.signal.aborted)throw fixedError('QODER_REQUEST_ABORTED');
    // One listing, one POST at most. No stale-catalog retry and no cross-request cache.
    const gateway=GATEWAYS[snapshot.region];
    const catalogURL=gateway+(snapshot.region==='cn'?'/algo/api/v2/model/list':MODELS_PATH);
    const listing=await fetch(catalogURL,{headers:cosyHeaders(catalogURL,snapshot,''),signal:controller.signal,redirect:'error'});
    if(!listing.ok){await listing.body?.cancel();return fixedResponse(502,'QODER_MODEL_DIRECTORY_UNAVAILABLE');}
    const limited=new Response(boundedBody(listing.body,MAX_MODEL_BYTES,abort));
    const listingBody=await limited.json();
    const models=modelInfos(snapshot.region==='cn'?{chat:listingBody.chat}:listingBody);
    const m=models.find(m=>m.key===chat.model);
    if(!m)return fixedResponse(400,'QODER_MODEL_UNAVAILABLE');
    if(controller.signal.aborted)throw fixedError('QODER_REQUEST_ABORTED');
    const url=gateway+CHAT_PATH;
    const wire=encodeBody(JSON.stringify(qoderBody(chat,m,snapshot.region)));
    const headers={...cosyHeaders(url,snapshot,wire),Accept:'text/event-stream','Cache-Control':'no-cache','X-Model-Key':m.key,'X-Model-Source':m.source};
    submitted=true;
    const upstream=await fetch(url,{method:'POST',headers,body:wire,signal:controller.signal,redirect:'error'});
    if(!upstream.ok){await upstream.body?.cancel();return fixedResponse(upstream.status>=400&&upstream.status<=599?upstream.status:502,'QODER_UPSTREAM_REJECTED');}
    const bounded=new Response(boundedBody(upstream.body,MAX_RESPONSE_BODY_BYTES,abort,()=>{transportBroken=true;}),{status:200});
    const response=await answer(bounded,chat,{abortUpstream:abort,releaseListener:release});
    if(controller.signal.aborted && signal?.aborted)throw fixedError('QODER_REQUEST_ABORTED');
    if(transportBroken)throw fixedError('QODER_OUTCOME_UNKNOWN');
    handed=true;
    return response;
  } catch(error) {
    if(signal?.aborted)throw fixedError('QODER_REQUEST_ABORTED');
    throw fixedError(submitted?'QODER_OUTCOME_UNKNOWN':'QODER_MODEL_DIRECTORY_UNAVAILABLE');
  } finally {if(!handed){abort();release();}}
}
export const _internal={modelInfo,qoderBody,decodeBody};
export const _protocol={events,answer,toolPolicy};
