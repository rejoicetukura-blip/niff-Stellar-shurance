# Requirements Document

## Introduction

The Deployment Registry is a backend service that persists authoritative mappings from environment/network to deployed Soroban contract addresses, semantic versions, activation timestamps, and expected WASM content hashes. It replaces static `.env` files as the source of truth when operating multiple environments (testnet, mainnet, local) or rotating deployments. The registry exposes admin-protected mutation endpoints, an append-only audit log mirroring on-chain admin events, and a read API consumed by the Next.js manifest loader.

## Glossary

- **Registry**: The backend subsystem (database tables + HTTP API) that stores and serves deployment records.
- **Deployment_Record**: A single row representing one contract deployment on one network, containing address, version, activation timestamp, and WASM hash.
- **Network**: A Stellar network identifier string, one of `mainnet`, `testnet`, or `local`.
- **Contract_Address**: A Stellar Strkey-encoded contract address (56-character `C…` string).
- **Wasm_Hash**: A lowercase hex-encoded SHA-256 hash of the deployed WASM binary (64 characters).
- **Semantic_Version**: A version string conforming to SemVer 2.0 (e.g. `1.2.3`).
- **Admin**: An authenticated operator holding a valid admin credential (API key or JWT) permitted to mutate deployment records.
- **Audit_Log**: An append-only table (`admin_audit_log`) recording every mutation with actor identity, timestamp, source IP, and a hash of the change payload.
- **Manifest**: A JSON document returned by the read API describing the active deployment(s) for a given network, consumed by the Next.js frontend.
- **Manifest_Loader**: The Next.js module that fetches the Manifest from the Registry read API at build time or runtime.
- **TTL**: Time-to-live duration for a cached read response.
- **API_Server**: The Express/Node.js backend process hosting the Registry HTTP endpoints.
- **DB**: The relational database (PostgreSQL) storing `deployment_records` and `admin_audit_log` tables.
- **Validator**: The request-validation layer inside the API_Server that checks input shapes and Stellar address formats before persistence.

---

## Requirements

### Requirement 1: Deployment Record Storage

**User Story:** As an operator, I want deployment records stored in a relational database with indexes on network and active flag, so that queries for active deployments on a given network are fast and consistent.

#### Acceptance Criteria

1. THE DB SHALL store each Deployment_Record with the fields: `id` (surrogate primary key), `network` (Network enum), `contract_address` (Contract_Address), `semantic_version` (Semantic_Version), `wasm_hash` (Wasm_Hash), `activated_at` (UTC timestamp), `is_active` (boolean), `created_at` (UTC timestamp), and `updated_at` (UTC timestamp).
2. THE DB SHALL enforce a unique constraint on `(network, contract_address)` to prevent duplicate address registrations per network.
3. THE DB SHALL maintain an index on `(network, is_active)` to support efficient active-deployment lookups.
4. WHEN a new Deployment_Record is inserted, THE DB SHALL set `created_at` and `updated_at` to the current UTC time automatically.
5. WHEN a Deployment_Record is updated, THE DB SHALL set `updated_at` to the current UTC time automatically.

---

### Requirement 2: Admin Authentication and Authorization

**User Story:** As a security engineer, I want all mutation endpoints protected by strong authentication, so that unauthorized clients cannot alter deployment records.

#### Acceptance Criteria

1. WHEN a request arrives at a mutation endpoint without a valid admin credential, THE API_Server SHALL reject the request with HTTP 401.
2. WHEN a request arrives at a mutation endpoint with a credential that is valid but lacks admin privileges, THE API_Server SHALL reject the request with HTTP 403.
3. THE API_Server SHALL accept admin credentials as a bearer token (JWT or opaque API key) supplied in the `Authorization` header.
4. WHERE the environment is `production`, THE API_Server SHALL require TLS on all inbound connections and SHALL reject plaintext HTTP requests with HTTP 301 redirect to HTTPS.
5. THE API_Server SHALL validate admin credentials against a secret stored in environment variables, not in source code or the DB.

---

### Requirement 3: Create Deployment Record (POST)

**User Story:** As an admin, I want to register a new deployment via a POST endpoint, so that the registry reflects newly deployed contracts.

#### Acceptance Criteria

1. WHEN an Admin submits a POST request to `/deployments` with a valid body, THE API_Server SHALL create a new Deployment_Record and return HTTP 201 with the created record.
2. WHEN the request body contains a `contract_address` that is not a valid Stellar Strkey contract address, THE Validator SHALL reject the request with HTTP 422 and a descriptive error message identifying the invalid field.
3. WHEN the request body contains a `semantic_version` that does not conform to SemVer 2.0, THE Validator SHALL reject the request with HTTP 422 and a descriptive error message.
4. WHEN the request body contains a `wasm_hash` that is not a 64-character lowercase hex string, THE Validator SHALL reject the request with HTTP 422 and a descriptive error message.
5. WHEN the request body contains a `network` value outside the set `{mainnet, testnet, local}`, THE Validator SHALL reject the request with HTTP 422 and a descriptive error message.
6. WHEN a POST request would violate the unique constraint on `(network, contract_address)`, THE API_Server SHALL return HTTP 409 with a descriptive conflict message.

---

### Requirement 4: Update Deployment Record (PATCH)

**User Story:** As an admin, I want to update an existing deployment record via a PATCH endpoint, so that I can rotate addresses, bump versions, or toggle the active flag without replacing the full record.

#### Acceptance Criteria

1. WHEN an Admin submits a PATCH request to `/deployments/:id` with a valid partial body, THE API_Server SHALL apply only the supplied fields to the Deployment_Record and return HTTP 200 with the updated record.
2. WHEN the PATCH body includes `contract_address`, THE Validator SHALL validate it as a Stellar Strkey contract address before applying the update.
3. WHEN the PATCH body includes `semantic_version`, THE Validator SHALL validate it against SemVer 2.0 before applying the update.
4. WHEN the PATCH body includes `wasm_hash`, THE Validator SHALL validate it as a 64-character lowercase hex string before applying the update.
5. WHEN a PATCH request targets a Deployment_Record `id` that does not exist, THE API_Server SHALL return HTTP 404.
6. WHEN a PATCH request would violate the unique constraint on `(network, contract_address)`, THE API_Server SHALL return HTTP 409.

---

### Requirement 5: Audit Logging

**User Story:** As a compliance officer, I want every admin mutation recorded in an append-only audit log, so that the audit trail can reconstruct who changed what and when.

#### Acceptance Criteria

1. WHEN an Admin mutation (POST or PATCH) succeeds, THE API_Server SHALL insert a row into `admin_audit_log` within the same database transaction as the Deployment_Record change.
2. THE DB SHALL store each audit row with: `id` (surrogate primary key), `actor` (the authenticated admin identity string), `action` (one of `create` or `update`), `target_id` (the affected Deployment_Record id), `change_payload_hash` (SHA-256 hex of the serialized change payload), `source_ip` (the client IP address string), and `occurred_at` (UTC timestamp).
3. THE DB SHALL prohibit DELETE and UPDATE operations on `admin_audit_log` rows via database-level permissions, enforcing append-only semantics.
4. WHEN the database transaction for a mutation fails, THE API_Server SHALL roll back both the Deployment_Record change and the audit row atomically, so no partial audit entries are persisted.
5. THE API_Server SHALL record the `source_ip` from the `X-Forwarded-For` header when present, falling back to the direct connection remote address.

---

### Requirement 6: Read API — Active Deployments

**User Story:** As a frontend developer, I want a read endpoint that returns active deployment records for a given network, so that the Next.js Manifest_Loader can fetch the correct contract addresses at runtime.

#### Acceptance Criteria

1. WHEN a GET request is made to `/deployments?network=<Network>`, THE API_Server SHALL return HTTP 200 with a JSON array of all Deployment_Records where `is_active = true` for the specified network.
2. WHEN the `network` query parameter is absent or invalid, THE API_Server SHALL return HTTP 422 with a descriptive error.
3. THE API_Server SHALL serve the GET `/deployments` endpoint without requiring admin credentials, permitting unauthenticated read access.
4. THE API_Server SHALL include a `Cache-Control` header with a TTL of 30 seconds on successful GET `/deployments` responses.
5. WHEN an Admin mutation succeeds, THE API_Server SHALL invalidate any in-process cache entries for the affected network so subsequent reads reflect the updated state within one TTL cycle.

---

### Requirement 7: Manifest Endpoint

**User Story:** As a frontend developer, I want a dedicated manifest endpoint that returns a structured JSON document, so that the Next.js Manifest_Loader can consume a stable, typed contract for building frontend configuration.

#### Acceptance Criteria

1. WHEN a GET request is made to `/manifest?network=<Network>`, THE API_Server SHALL return HTTP 200 with a JSON object containing a `network` field and a `contracts` array, where each element includes `contract_address`, `semantic_version`, `wasm_hash`, and `activated_at`.
2. WHEN the `network` query parameter is absent or invalid, THE API_Server SHALL return HTTP 422 with a descriptive error.
3. THE API_Server SHALL serve GET `/manifest` without requiring admin credentials.
4. THE API_Server SHALL set `Content-Type: application/json` on all `/manifest` responses.
5. WHEN no active Deployment_Records exist for the requested network, THE API_Server SHALL return HTTP 200 with a JSON object containing an empty `contracts` array, not HTTP 404.

---

### Requirement 8: Input Validation — Stellar Address Format

**User Story:** As a developer, I want the API to validate Stellar contract addresses strictly, so that malformed addresses are never persisted and on-chain lookups always succeed.

#### Acceptance Criteria

1. THE Validator SHALL accept a `contract_address` only if it is a 56-character string beginning with `C` and passing Stellar Strkey checksum validation.
2. WHEN a `contract_address` fails Strkey checksum validation, THE Validator SHALL return a descriptive error message that identifies the field and states the expected format.
3. THE Validator SHALL reject `contract_address` values that are valid Strkey account addresses (beginning with `G`) rather than contract addresses (beginning with `C`).

---

### Requirement 9: Development Seed Data

**User Story:** As a developer, I want seed data for local and testnet environments applied via migration scripts, so that the development environment is immediately usable without manual database setup.

#### Acceptance Criteria

1. THE DB SHALL include a seed migration that inserts at least one Deployment_Record for `network = local` and one for `network = testnet` with placeholder `contract_address`, `semantic_version`, `wasm_hash`, and `activated_at` values.
2. WHEN the seed migration is run in a `production` environment, THE API_Server SHALL skip the seed insertion and log a warning, leaving production data unmodified.
3. THE DB seed migration SHALL be idempotent: running it multiple times SHALL NOT create duplicate Deployment_Records.

---

### Requirement 10: Release Runbook Integration

**User Story:** As a DevOps engineer, I want documented procedures for updating the registry as part of a contract release, so that deployments are consistently recorded and the registry stays in sync with on-chain state.

#### Acceptance Criteria

1. THE Registry documentation SHALL describe the sequence of steps an operator must follow after deploying a new contract version: obtain the new `contract_address` and `wasm_hash`, call the POST endpoint, verify the response, and deactivate the previous record via PATCH.
2. THE Registry documentation SHALL specify the environment variables required for admin authentication in each environment (`local`, `testnet`, `mainnet`).
3. THE Registry documentation SHALL include an example `curl` command for each mutation endpoint showing required headers and a valid request body.
