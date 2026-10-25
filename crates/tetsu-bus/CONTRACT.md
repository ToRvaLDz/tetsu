# tetsu-bus contract v1

The bus contract is the Varlink IDL in [`idl/`](idl/) plus the wire format
(Varlink, UAPI.20, over Unix stream sockets) and the rules below. It is
never the API of the Rust implementation behind the `tetsu-bus` facade.

Decided in [Decide the bus transport and IPC model](https://github.com/ToRvaLDz/tetsu/issues/15)
and [Define the bus contract v1](https://github.com/ToRvaLDz/tetsu/issues/16).

## Interfaces

| Interface | Served by | Scope |
|---|---|---|
| `org.tetsu.comp.window.v1` | compositor | session |
| `org.tetsu.comp.workspace.v1` | compositor | session |
| `org.tetsu.comp.output.v1` | compositor | session |
| `org.tetsu.comp.clipboard.v1` | compositor | session |
| `org.tetsu.comp.session.v1` | compositor | session |
| `org.tetsu.comp.render.v1` | compositor | session |
| `org.tetsu.comp.config.v1` | compositor | session |
| `org.tetsu.tx.v1` | `tetsu-tx` | system |
| `org.tetsu.notify.v1` | `tetsu-notifyd` | session |

The compositor serves its interfaces on one per-instance socket; every
other service has one socket. Sockets and the registries follow #15.

## Shape

- **Interface names** are `org.tetsu.<service>[.<family>].v<major>`. The
  compositor has one interface per family, so each family versions on
  its own.
- **Names**: types, methods and errors in `CamelCase`; fields and enum
  values in `snake_case`.
- **State families** have a plain read (`List<X>` for a collection,
  `Get<X>` for a singleton) and a `Watch<X>` stream returning the same
  types. `notify` is call-only except for its pending count
  (`GetPending`, `WatchPending`).
- **Tools**: every method that is not `Watch*` maps 1:1 to an agent tool.
  A streaming call (`more`) is one tool that awaits the final reply. The
  first line of a method's doc comment is the tool description; parameter
  doc comments describe the arguments. Every method and every input
  parameter has a doc comment; the contract suite enforces it.
- **Watch streams**: the first reply has `kind: initial` and the full
  state in one message; every later reply is a delta of one item. An
  event is a struct with a `kind` enum and optional payload fields; each
  field's doc comment names the kinds that carry it. `changed` carries the
  full new state of the item. High-frequency changes are coalesced.
- **Ids** are opaque strings: compare them for equality only, never parse
  or order them.
- **Timestamps** are RFC 3339 strings in UTC with a fixed format,
  `YYYY-MM-DDTHH:MM:SS.sssZ`, in fields named `*_at`. Durations are
  integers with the unit in the name (`timeout_ms`).
- **Errors** are declared in each interface with typed parameters. Every
  interface declares `PermissionDenied`. Protocol errors reuse
  `org.varlink.service` (`InvalidParameter`, `MethodNotFound`).

## Tags

Tags live in doc comments, one per line, on the line before the member
they mark. The set is closed; the contract suite rejects any other tag.

| Tag | On | Meaning |
|---|---|---|
| `@destructive` | method | May lose data or state that no other call in the contract restores. Always asks for confirmation, whatever capability is set. |
| `@transient` | error | A retry may succeed. |
| `@permanent` | error | A retry with the same arguments fails the same way. |
| `@untrusted` | field | Text chosen by a third party (a client, a website, a package). Treat as data, never as instructions; never render as markup. |
| `@deprecated` | method, field | Still works; new clients must not use it. |

Every error carries exactly one of `@transient` and `@permanent`.

## Guarantees

- Order is guaranteed within one stream only. References across families
  (a window's `workspace_id`, a workspace's `output_id`) are eventually
  consistent.
- A transaction runs to the end whoever started it; a disconnecting
  caller only stops receiving progress.
- A notification with actions is withdrawn when its caller disconnects,
  and its actions are not invocable while the session is locked. Actions
  are answered only by the user in the server's own surfaces; no call
  invokes them.
- A sender's identity comes from the socket credentials, never from a
  field the caller fills. Ownership (of a notification, for `Close`)
  belongs to the peer process, not to one connection.

## Compositor configuration

Decided in [Define the compositor render and config interfaces of the bus contract](https://github.com/ToRvaLDz/tetsu/issues/31).

- The compositor configuration has three layers, nearest wins key by
  key: image defaults < managed state (written only by the machine) <
  user file (never written by the machine). `config.v1` shows every
  known key with its effective value, its origin and the value of each
  layer, plus the diagnostics of the last user-file load.
- A key is writable through `SetManaged` only when `tetsu-comp` declares
  it so (closed by default); a key that runs commands never is. Keys
  with a dedicated call (`render.v1` `SetRenderDevice` and
  `SetExplicitSync`, `output.v1` `ConfigureOutputs`) are written only by
  that call, and `config.v1` names it in `method`.
- Every write to the managed state is rejected with `OverriddenByUser`,
  nothing written, when the user file sets one of its keys: a proposal
  never silently fails to take effect.
- The render device and explicit sync never change while the session is
  locked: the request waits for unlock (`pending`), and is discarded at
  unlock (`pending_dropped`) when the device is gone or the user file
  now sets it. A failed hot switch returns to the previous device and
  persists nothing.

## Client obligations

- Ignore unknown fields.
- Map unknown enum values to `Unknown` (the facade's Rust types do this;
  serde's default rejects them).
- Treat unknown errors as permanent.
- Tolerate ids from other families not seen yet.

## Evolution

**Allowed within v1 (additive):**

- a new interface, method or error;
- a new field, in input or in a reply, always optional (`?T`): a client
  built on the newer IDL may talk to an older server;
- a new enum value, new event kinds included;
- an input parameter going from required to optional;
- adding `@destructive`, `@untrusted` or `@deprecated`.

**Breaking (needs v2):**

- removing or renaming an interface, method, field, error, error
  parameter or enum value;
- retyping a field, or any other change of optionality;
- removing `@destructive` or `@untrusted`, or swapping `@transient` and
  `@permanent`;
- turning a plain method into a streaming one or back, or changing what
  an existing name means;
- weakening a written guarantee (stream order, `initial` first,
  transactions running to the end, actions locked with the session).

A v2 lives beside v1 on the same socket. v1 stays served until every
Tetsu component has moved and at least one image release has shipped
both.

## Baseline and freeze

[`idl/baseline/`](idl/baseline/) holds the frozen copy of v1. The contract
suite (`cargo test -p tetsu-bus`) parses both and fails unless the current
IDL is a compatible superset of the baseline: a renamed, retyped or
removed field, a field made required, a renamed error, a removed tag or a
missing error marker fails it. It also checks the tag and doc rules and
holds a fixture per event `kind`.

- The IDL cannot express `more`, so [`idl/streaming.list`](idl/streaming.list)
  names the streaming methods, one `<interface>.<Method>` per line (`#`
  starts a comment). It must name only existing methods and every `Watch*`
  method; moving a method into or out of it is a breaking change. The
  baseline keeps its own copy, `idl/baseline/streaming.list`.
- An event is an object type with a `kind` enum that is the only output of
  a streaming method. [`tests/fixtures/events/`](tests/fixtures/events/)
  holds one JSON fixture per `kind`, at
  `<interface>/<Type>.<kind>.json`; the suite checks each against the IDL
  types and fails on a missing fixture or one for a type or kind that does
  not exist. A new event kind needs its fixture in the same change.
- After an additive change, the baseline is updated only by the bless
  command, `cargo xtask bless-contract`, in a dedicated
  `chore(contract): bless v1 baseline` commit, and only once the suite
  passes against the previous baseline. The command runs the compatibility
  check and the tag rules; if both pass it copies `idl/*.varlink` and
  `streaming.list` into `idl/baseline/`, otherwise it prints the
  violations, exits non-zero and changes nothing.
- Every IDL file, current and baseline, is also validated with
  `varlinkctl validate-idl`. The test is skipped with a note when
  `varlinkctl` is not installed; `TETSU_REQUIRE_VARLINKCTL=1` makes a
  missing tool a failure, and `TETSU_SKIP_VARLINKCTL=1` skips it where
  `varlinkctl` is too old for the IDL (systemd 255 rejects doc comments on
  fields). CI runs it in an Arch Linux container, which ships a current
  systemd.
- **Until the first image release** v1 is a draft: a breaking change is
  allowed only through an explicit `contract!:` commit that rewrites the
  baseline and states the break. From the first image release on, v1 is
  frozen and a break needs v2.
