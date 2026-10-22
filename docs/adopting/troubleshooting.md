# Troubleshooting

Find the words on your screen. Each row says what the layer meant and what to do.

## The tool is greyed out

| What `status` says | What happened | Fix |
|---|---|---|
| `no model is chosen for 'select-subject'` | Nothing is bound for that tool, in the suite or for this app. | Open the popup (`ToolDesk::missing_model`) or Set up AI, and choose a model. This is normal on a fresh install. |
| `This app can't run the chosen model yet. Choose another in Set up AI.` | The chosen connection's provider name has no adapter registered in this app's `ProviderFactory`. | Register the adapter, or choose a model this app can run. Check the provider name matches exactly (it is not case sensitive). |
| `connection <id> is switched off` | The connection exists but is disabled. | Switch it on in Set up AI. |
| `connection <id> no longer exists` | The binding points at a deleted connection. | Choose again. `ToolDesk::clear` removes a stale default. |
| `connection <id> does not offer model <m>` | The model was removed from the connection. | Choose another model. |
| `model <m> cannot do <capability>` | The bound model does not offer the tool's capability. | Choose a model that does. |
| `model <m> does not accept a '<input>' input` | Your tool sends an input the model cannot take. | Choose another model, or stop sending that input if the tool does not need it. |
| `tool <id> does not match its shared definition` | Another app registered this id with a different capability or inputs. | Use the registered meaning, or pick a new id. |
| `no key is saved for connection "<label>"; add it in the ArtCraft Launcher` | The connection needs a key and the key store has none. | Add the key in the launcher or your Preferences. |
| `connections file: ...` or `no place to keep the connections file on this system` | The shared store could not be read, or the OS gave no config folder. | Check the folder, or set `CRAFT_AI_DIR`. |

The "model cannot take your input" cases are a feature. They keep a model from being offered for a
tool it cannot serve. Do not work around them by sending less than the tool needs.

## `plan` or `start` fails

| Error | Cause | Fix |
|---|---|---|
| `ConsentRequired`: `sending to <host> needs your agreement first` | The connection is remote and the ledger does not cover this app, destination, capability and kind of data. | Show the disclosure, call `ledger.agree(...)`, and start with the ledger it returns. |
| Consent asked again and again | The term was "this time", the term lapsed, the ledger was not saved, or the destination or capability changed. | Save the ledger when `term.remembers()`. Keep it in `client.consent_file()`. |
| `Unsupported`: `this model does not accept a '<input>' input` | The request carries an input the model's spec lacks. | Send only declared inputs. |
| `Unsupported`: `this model accepts at most N '<input>' input(s)` | Too many of one input. | Send fewer, or choose another model. |
| `InvalidRequest`: `a '<input>' input is required` | A required input is missing. | Send it. |
| `InvalidRequest`: `this request is for <a>, but "<tool>" needs <b>` | The request's capability differs from the tool's. | Build the request from `role.capability`. |
| `Unsupported`: `this model returns at most N output(s)` | `with_outputs` asked for too many. | Ask for fewer. |
| `Unsupported`: `this model cannot stream` | `streaming()` on a model that does not stream. | Remove it. |
| `InvalidRequest` on a payload | A payload is too big (32 MiB inline by default) or has a bad media type. | Use `Payload::handle` for large data. Media types are lower-case `type/subtype`. |

## The job misbehaves

| Symptom | Cause | Fix |
|---|---|---|
| Events seem to vanish | Each poll returns only new events. | Collect them in every loop. Do not poll just to read the state. |
| `the job took too long` | `Job::wait` hit its timeout. It cancelled the job. | Use your own polling and a longer limit, or a different model. |
| The interface freezes on Start | `start` ran on the interface thread. | Move `start` and `poll` to a worker. |
| Cancel does nothing | Targeted cancel is off by default for some ComfyUI servers, because an older server may treat it as a global interrupt. | Expect the job to finish, and discard its result. |
| The result is empty but the job succeeded | The tool has a fixed result shape (selection does) and no match is a valid answer. | Read it with the contract: `segment::read_found` says whether anything matched. |
| `Auth` after the key is saved | The wrong key name, or the environment variable shadows the keychain. | Variables named `CRAFT_AI_SECRET_...` are read first. Unset the one in the way. |
| `RateLimited` | The service asked you to slow down. | Wait `retry_after_secs` when present. |

## My tests

| Symptom | Cause | Fix |
|---|---|---|
| A test changes my real settings | The test used `AiClient::standard`. | Use `AiClient::new` with a temporary `SharedStore`, or set `CRAFT_AI_DIR` for that test only. |
| `status` is `Ready` but the real tool does nothing | Ready means the route resolves. It does not mean a model ran. | Run the model through your tool and check the result in the document. |
| Two apps in a test disagree | Each client has its own `ProviderFactory`. An app without the adapter is `Unavailable`. | That is the point. Register the adapter in each client that should run it. |
| `register_tool` says the name is in use | A built-in tool, or an earlier tool, has that id. | Choose another id, or use the existing tool. |

If nothing here matches, run `client.status(&role)` and read the reason first. It is written to be
the answer.
