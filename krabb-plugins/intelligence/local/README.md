# Local Server

This provider is optional and is distributed separately from the
core release package.

Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- local
```

It talks to any inference server that offers the OpenAI-compatible Chat
Completions API, with streaming, images, and tool calls. Ollama keeps its own
plugin.

## Servers

With `KRABB_LOCAL_URL=auto`, the default, Krabb asks each usual address in this
order and uses the first server that lists its models:

| Server | Address |
| --- | --- |
| vLLM | `http://127.0.0.1:8000/v1` |
| llama.cpp (`llama-server`) | `http://127.0.0.1:8080/v1` |
| LM Studio | `http://127.0.0.1:1234/v1` |
| Jan | `http://127.0.0.1:1337/v1` |
| KoboldCpp | `http://127.0.0.1:5001/v1` |
| SGLang | `http://127.0.0.1:30000/v1` |

Any other compatible server, such as LocalAI, Text Generation Inference, or a
GPU machine on your network, works when you set its address with
`krabb local setup`, for example `http://192.168.1.20:8000/v1`. Plain HTTP is
accepted only for this machine, private addresses, and local network names such
as `gpu-box` or `gpu-box.local`; any other host needs HTTPS. A name without a dot is
trusted as local, so make sure your network's search domain does not send it elsewhere.

When Krabb finds a server on its own, it remembers it until a request fails and never
sends `KRABB_LOCAL_KEY` there; set `KRABB_LOCAL_URL` to send the key.

## Setup

| Setting | Default | Meaning |
| --- | --- | --- |
| `KRABB_LOCAL_URL` | `auto` | The server's base URL, ending in `/v1` |
| `KRABB_LOCAL_KEY` | none | An API key, only for a server started with one |
| `KRABB_LOCAL_CONTEXT_LENGTH` | `auto` | Tokens of context; `auto` reads it from the server, else 4096 |

llama.cpp does not report the context it was started with (`-c`), so set
`KRABB_LOCAL_CONTEXT_LENGTH` to match it.

`krabb local models` lists the models the server offers. The `default` model is
the first one it lists; choose another with `/model` or `krabb config`.

## Tool calls

Krabb acts through tool calls and never emulates them. Start the server with tool
calling enabled: vLLM needs `--enable-auto-tool-choice` and a `--tool-call-parser`
for the model, and llama.cpp needs `--jinja`. When a server refuses tool calls,
Krabb says so instead of answering without them.
