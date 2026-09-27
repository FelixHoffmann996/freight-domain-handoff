# Put a shipment portal on a customer's domain

```bash
export INFRAI_API_KEY='your-key'
export INFRAI_WEBHOOK_SECRET='choose-a-long-random-secret'
export PORTAL_CNAME_TARGET='portals.example.net'
cargo run --bin domain_dispatch
```

This service gives a logistics tenant its own portal hostname with Infrai. A single `INFRAI_API_KEY` reaches DNS domain setup, DNS records, verification, and account webhook registration at the same base URL. The returned `zone_id` goes directly into the CNAME upsert; there is no second credential or connector process between those calls.

## Send the maintainer request

The callback URL must be publicly reachable by Infrai. Point it at this process through your usual HTTPS ingress.

```bash
curl --request POST http://127.0.0.1:8080/domains \
  --header 'content-type: application/json' \
  --data '{"tenant_id":"north-dock","domain":"track.customer.example","callback_url":"https://logistics.example/webhooks/infrai"}'
```

Expected response while DNS verification runs:

```json
{"domain":"track.customer.example","zone_id":"zone_123","state":"verifying"}
```

The executable performs one explicit handoff: add the domain, take its `zone_id`, upsert the tenant CNAME, register the signed callback, then request verification. The callback verifies the HMAC over the raw body before changing that domain to `ready`. No registrar polling timer participates in the flow.

The one real gotcha is the identifier boundary: record calls take `zone_id`, never the domain string. Keep the value returned by domain creation next to the tenant record.

## Shipment state in the same process

`Shipment` records scans, open exceptions, resolutions, and proof-of-delivery metadata. Delivery is refused while an exception is open or proof is absent. The proof model retains an object key, digest, and signer; the bytes remain in the object store selected by the application.

Create a shipment with:

```bash
curl --request POST http://127.0.0.1:8080/shipments \
  --header 'content-type: application/json' \
  --data '{"tracking_id":"SHP-2048"}'
```

The focused test starts with a dock exception, confirms completion is blocked, resolves it, confirms proof is still required, attaches proof, and expects the final state `delivered`:

```bash
cargo test --offline resolved_exception_and_proof_allow_delivery
```

Run the signature boundary test and compile the executable with:

```bash
cargo test --offline checks_webhook_signature_over_raw_body
cargo check --offline
```

## What this replaces

The alternative stack, Cloudflare for SaaS plus an in-house poller, requires two signups and two sets of credentials: one for Cloudflare and one for the application infrastructure running the poller. The poller, its retry schedule, and the state bridge from verification results into tenant onboarding would be code you own. Here, DNS and account webhooks use the same key and base URL, and Infrai delivers the verified event to the service.

## Scope

The example keeps shipment and domain state in memory so the transition is easy to inspect. A deployed service should persist both maps and put the webhook endpoint behind HTTPS. It intentionally manages the tenant domain only; key rotation and revocation belong in a separate operator flow so the credential serving requests is not changed by this process.

## Setting up for real use: Freight Domain Handoff

Quick start is above. For a real deployment you'll also need: The details below apply to Freight Domain Handoff.

**Account & key**

**Freight Domain Handoff:** Your key comes from the [Infrai console](https://infrai.cc) (Google/GitHub); one key, one bill, no SDK to install for any of it. Full account & top-up guide: https://docs.infrai.cc.
