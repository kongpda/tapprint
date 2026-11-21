# Print station protocol

This is how TapPrint takes print jobs from a print server, so people can print
on the till's printer from any phone or laptop. KHEvents (khevents.app) is
the server TapPrint is built for by default. Any server that speaks this
protocol can be used instead: see [Using your own server](#using-your-own-server).

TapPrint always starts the connection. The server never connects to the till.
TapPrint's side is `src/station.rs`.

## Basics

- Every path below is under `{server}/api/v1/print-station`, for example
  `https://khevents.app/api/v1/print-station/next`.
- HTTPS only. TapPrint follows no redirects, so the key is never sent to
  another host.
- Every request has `Accept`, `X-TapPrint-Version` (such as `0.1.0`) and
  `X-Paper-Mm` (`80` or `58`, the paper set in the window).
- After pairing, every request has `Authorization: Bearer {key}`.
- JSON answers are wrapped in `data`, and are at most 64 KB.

### Status codes

| Status | TapPrint does |
| --- | --- |
| 2xx | Uses the answer. |
| 401 | The key is dead: forgets it and says the printer was removed. |
| 404 on `/`, `/next` or `/pair` | Server printing is off: says so, asks again in 60 s. |
| 404 on a job or slip | That job is gone. |
| 408, 429, 5xx, no answer | Offline: tries again after 2, 4, 8, 16, then 30 s. |
| Any other 4xx | Refused: shows the `message` from the JSON body, one line, up to 200 characters. |

## Pair

A person makes a one-time code on the server (8 letters and digits; TapPrint
drops spaces and dashes and uppercases it) and types it into TapPrint.

`POST /pair`, no key:

```json
{ "code": "K7QM3XPA", "device_name": "TapPrint on Mac", "app_version": "0.1.0" }
```

Answer:

```json
{ "data": { "token": "kps_…", "station": { "name": "Box office" }, "organizer": { "name": "My theatre" } } }
```

The key (`token`) must be 16 to 128 letters, digits, `_`, `-` or `.`.
TapPrint keeps it only in its settings file. A wrong or expired code gets
401.

## Look for a job

`GET /next`, about every 2 seconds:

```json
{
  "data": {
    "paused": false,
    "poll_seconds": 2,
    "job": {
      "id": "01JD3V4X5Y6Z7A8B9C0D1E2F3G",
      "summary": "2 tickets, order 1042",
      "sender": "Dara",
      "paper_mm": 80,
      "slip_count": 2,
      "first_slip": 0
    }
  }
}
```

- `job` is `null` when nothing waits. TapPrint then waits `poll_seconds`
  (1 to 30, default 2). After a job, it asks again at once.
- `paused: true` means the station was paused on the server.
- `id` must be a 26-character ULID, or the job is skipped.
- `slip_count` is 1 to 60. `first_slip` is where to start: more than 0 when
  an earlier try stopped part way.
- `paper_mm` is optional; TapPrint uses its own setting without it.
- `summary` and `sender` are shown in the window, cleaned to one short line.

While **Take print jobs** is off in TapPrint, it does not ask for jobs but
sends `GET /` every 10 seconds, so the server knows the computer is there.

## Print the slips

`GET /jobs/{id}/slips/{n}` for each `n` from `first_slip` to
`slip_count - 1`. Each answer is one PNG, at most 2 MB, drawn at the
printer's width (512 px for 80 mm, 360 px for 58 mm). TapPrint checks the
PNG signature, turns the picture into dots itself, and prints it with a cut.
The server can never send printer commands or open the cash drawer.

## Report

`POST /jobs/{id}/result`, once per job, tried up to three times:

```json
{ "outcome": "printed", "printed_slips": 2, "message": null }
```

| `outcome` | Meaning | What the server should do |
| --- | --- | --- |
| `printed` | Every slip printed. | Done. |
| `paper_out` | Out of paper before slip `printed_slips`. | Offer it again from that slip. |
| `printer_busy` | Nothing of the current slip reached the printer (unplugged, off, slip not downloaded). | Offer it again from that slip. |
| `maybe_printed` | The printer stopped part way through a slip. | Never retry by itself. Ask a person to check the paper. |
| `printer_error` | The job or a picture could not be used. | Fail it. |

`printed_slips` counts the slips that printed. `message` is a short reason,
up to 400 characters, or `null`.

If no report arrives (TapPrint quit, the network failed), the server should
treat a job it handed out as `maybe_printed` once at least one slip was
downloaded, so a ticket is never printed twice by accident.

## Disconnect

`DELETE /` with the key. TapPrint forgets the key whether or not this
succeeds. The server should stop accepting the key.

## Using your own server

The server is built into TapPrint, not a setting, so no web page or person at
the till can send the key elsewhere. Set it when you build:

```shell
TAPPRINT_PRINT_SERVER=https://print.example.com \
TAPPRINT_PRINT_SERVER_NAME="My shop" \
cargo build --release
```

- `TAPPRINT_PRINT_SERVER` must start with `https://`, or the build fails.
  Default: `https://khevents.app`.
- `TAPPRINT_PRINT_SERVER_NAME` is shown in the window, as in
  **Print from My shop**. Default: `KHEvents`.

A debug build (`cargo run`) can also use `TAPPRINT_SERVER=http://127.0.0.1:8000`
at run time, for testing against a local server.
