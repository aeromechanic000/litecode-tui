---
name: translate
description: Free keyless translation via the MyMemory API — verified endpoint, params, response shape, and a known-good one-liner so you don't guess
trigger: translate, translation, translator, 翻译, 译成, 翻译成
---

When the task is to translate text via an online API, use **MyMemory** — it is
free, needs no API key, and uses a simple GET. Don't reach for Google Translate
(unreachable behind the regional network block, and its `translate_a/single`
endpoint returns a nested array that's easy to parse wrong) and don't invent
endpoints from memory or search snippets.

## Verified API — use it verbatim, don't improvise

- **Endpoint:** `https://api.mymemory.translated.net/get`
  - Mind the `api.` subdomain and the `/get` path. These are all WRONG and return
    404 / 401 / HTML: `mymemory.translated.net`, `mymemory.dev`,
    `api.mymemory.dev`, `…/api`, `…/api/translate`.
- **Method:** `GET`
- **Params:**
  - `q` — the text to translate (MUST be URL-encoded at runtime).
  - `langpair` — `source|target`, e.g. `zh|en`, `zh-CN|en`, `en|it`.
    This is `langpair`. NOT `source`/`target`, NOT Google's `sl`/`tl`,
    NOT `hl`/`target`.
- **Response:** JSON; the translation is at `responseData.translatedText`.
- **Quota:** ~5000 chars/day anonymous; add `de=you@email` for a higher limit.

## Known-good one-liner (Python, stdlib only — no `requests` dependency)

```python
import urllib.request, urllib.parse, json
url = "https://api.mymemory.translated.net/get?q=" + urllib.parse.quote("周末采购生鲜食材") + "&langpair=zh|en"
print(json.loads(urllib.request.urlopen(url, timeout=10).read().decode())["responseData"]["translatedText"])
```

## Rules — these are the exact mistakes that break this task

- **Never put raw non-ASCII (e.g. Chinese) directly in a URL.** The HTTP layer
  encodes the request line as ASCII and raises `UnicodeEncodeError`. Build the
  query with `urllib.parse.quote` / `urlencode` so it is encoded at runtime.
- **Never hand-type `%E5%91%AB…` percent-encodings** — you will get them wrong and
  they drift between attempts. Let the encoder produce them.
- Use the endpoint / params / field above verbatim. Without this card, a 9B model
  repeatedly invented plausible-but-wrong hosts and params — don't "improve" on
  the verified spec.
- Run the snippet **once** to verify. If it fails with a **timeout, connection
  refused, HTTP 401/403, or a non-JSON / HTML body**, that is the regional network
  block (see the *Network-blocked regions* rule) — do NOT swap endpoints and retry
  in a loop. Report the block plainly. A real result is JSON with
  `responseData.translatedText`; an HTML page, empty body, or 401 is a block or
  error, not something to feed to `json.loads`.

## Second option (only if MyMemory is blocked and a self-hosted instance exists)

**LibreTranslate** requires **POST** with a JSON body
(`{"q": "...", "source": "zh", "target": "en"}`), not GET, and public instances
move often / may require a key. For a keyless GET call, MyMemory is the default.
