# Implementation Plan: Deployment Registry

## Overview

Implement the Deployment Registry as a TypeScript/Express backend service with PostgreSQL persistence, an in-process cache, admin auth middleware, input validators, and a Next.js manifest loader. Tasks are ordered so each step builds on the previous and nothing is left unwired.

## Tasks

- [ ] 1. PostgreSQL migrations
  - [ ] 1.1 Create `001_create_deployment_records.sql`
    - Define `network_enum` type and `deployment_records` table with all columns from the data model
    - Add unique index `uq_network_address` on `(network, contract_address)`
    - Add index `idx_network_active` on `(network, is_active)`
    - Add `set_updated_at()` trigger function and `trg_deployment_records_updated_at` trigger
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5_
  - [ ] 1.2 Create `002_create_admin_audit_log.sql`
    - Define `admin_audit_log` table with all columns from the data model
    - Add `CHECK (action IN ('create', 'update'))` constraint
    - Add `REVOKE UPDATE, DELETE` statements to enforce append-only semantics
    - _Requirements: 5.2, 5.3_
  - [ ] 1.3 Create `003_seed_dev_data.sql`
    - Insert one `local` and one `testnet` placeholder record using `ON CONFLICT DO NOTHING`
    - Wrap in a `DO $$ BEGIN IF ... THEN RAISE WARNING ... RETURN; END IF; ... END; $$` block that checks `app.environment`
    - _Requirements: 9.1, 9.2, 9.3_

- [ ] 2. TypeScript types and DTOs
  - [ ] 2.1 Create `backend/src/types.ts`
    - Define `Network` union type, `DeploymentRecord`, `AuditLogRow`, `CreateDeploymentDto`, `PatchDeploymentDto` interfaces
    - Define `ValidationError` class with `field` and `message` properties
    - _Requirements: 1.1, 3.1, 4.1_

- [ ] 3. Input validators
  - [ ] 3.1 Create `backend/src/validators/deployment.ts`
    - Implement `validateContractAddress` using `@stellar/stellar-base` Strkey checksum (56-char, `C` prefix, rejects `G` prefix)
    - Implement `validateSemVer` using `semver` package's `valid()` function
    - Implement `validateWasmHash` using regex `/^[0-9a-f]{64}$/`
    - Implement `validateNetwork` as a type guard against `['mainnet', 'testnet', 'local']`
    - Implement `validateCreateBody` and `validatePatchBody` that throw `ValidationError` on failure
    - _Requirements: 3.2, 3.3, 3.4, 3.5, 4.2, 4.3, 4.4, 8.1, 8.2, 8.3_
  - [ ] 3.2 Write property tests for validators
    - **Property 4: Strkey contract address validation** — `fc.string()` inputs; assert accept iff 56-char `C`-prefix valid Strkey, reject `G`-prefix
    - **Property 5: SemVer validation rejects non-conforming version strings**
    - **Property 6: WASM hash validation rejects non-hex or wrong-length strings**
    - **Property 7: Network enum validation rejects unknown network values**
    - **Validates: Requirements 3.2, 3.3, 3.4, 3.5, 4.2, 4.3, 4.4, 8.1, 8.3**
    - Tag each test with `// Feature: deployment-registry, Property N: ...`
    - Use `{ numRuns: 100 }` for all `fc.assert` calls

- [ ] 4. Admin auth and TLS middleware
  - [ ] 4.1 Create `backend/src/middleware/adminAuth.ts`
    - Read `Authorization: Bearer <token>` header; return 401 if absent/malformed
    - Compare token against `process.env.ADMIN_API_KEY` using `crypto.timingSafeEqual`; return 403 on mismatch
    - _Requirements: 2.1, 2.2, 2.3, 2.5_
  - [ ] 4.2 Create `backend/src/middleware/requireHttps.ts`
    - No-op when `NODE_ENV !== 'production'`
    - Check `req.secure` or `X-Forwarded-Proto: https`; issue HTTP 301 redirect to HTTPS equivalent otherwise
    - _Requirements: 2.4_
  - [ ] 4.3 Write unit tests for auth and TLS middleware
    - `adminAuth`: missing header → 401, wrong token → 403, correct token → `next()` called
    - `requireHttps`: plaintext in production → 301, HTTPS in production → `next()`, any in non-production → `next()`
    - **Property 3: Auth middleware rejects requests without valid admin credentials**
    - **Validates: Requirements 2.1, 2.2, 2.3, 2.4**

- [ ] 5. In-process cache module
  - [ ] 5.1 Create `backend/src/cache/deploymentCache.ts`
    - Implement `Map<Network, CacheEntry>` store with `{ data, expiresAt }` entries
    - Implement `getCached(network)` returning `null` when entry is absent or `Date.now() > expiresAt`
    - Implement `setCached(network, data)` setting `expiresAt = Date.now() + 30_000`
    - Implement `invalidate(network)` deleting the entry for that network
    - _Requirements: 6.4, 6.5_

- [ ] 6. Database client and migration runner
  - [ ] 6.1 Create `backend/src/db/client.ts`
    - Export a `pg.Pool` instance configured from `DATABASE_URL` environment variable
    - _Requirements: 1.1_
  - [ ] 6.2 Create `backend/src/db/migrate.ts`
    - Read SQL files from `migrations/` directory in order
    - Skip `003_seed_dev_data.sql` when `NODE_ENV === 'production'` and log a warning
    - Execute each migration in a transaction; track applied migrations in a `schema_migrations` table
    - _Requirements: 9.2_

- [ ] 7. POST /deployments route handler
  - [ ] 7.1 Create `backend/src/routes/deployments.ts` with `createDeployment` handler
    - Call `validateCreateBody`; catch `ValidationError` and return 422
    - Open a `pg` transaction: INSERT into `deployment_records`, INSERT into `admin_audit_log`
    - Compute `change_payload_hash` as SHA-256 hex of the serialized request body
    - Extract `source_ip` from `X-Forwarded-For` header with fallback to `req.socket.remoteAddress`
    - On unique constraint violation (`23505`) return 409; on other DB errors return 500
    - On commit: call `invalidate(network)`, return 201 with the created record
    - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 5.1, 5.2, 5.4, 5.5_
  - [ ] 7.2 Write property tests for POST /deployments
    - **Property 1: Unique constraint prevents duplicate (network, contract_address) pairs** — generate valid DTOs with same network+address, assert second POST returns 409
    - **Property 8: POST creates a record and returns it** — generate valid `CreateDeploymentDto`, assert 201 and response fields match submitted values
    - **Property 10: Mutation atomically writes audit log row** — assert audit row exists after success; assert no audit row after forced rollback
    - **Property 11: Source IP recorded from X-Forwarded-For with fallback**
    - **Validates: Requirements 3.1, 3.6, 5.1, 5.2, 5.4, 5.5**

- [ ] 8. PATCH /deployments/:id route handler
  - [ ] 8.1 Add `updateDeployment` handler to `backend/src/routes/deployments.ts`
    - Call `validatePatchBody`; catch `ValidationError` and return 422
    - Open a `pg` transaction: UPDATE `deployment_records` for the given `id`, INSERT into `admin_audit_log`
    - Return 404 when UPDATE affects 0 rows; return 409 on unique constraint violation
    - On commit: call `invalidate(network)`, return 200 with the updated record
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 5.1, 5.4_
  - [ ] 8.2 Write property tests for PATCH /deployments/:id
    - **Property 9: PATCH applies only supplied fields** — generate existing record + partial DTO, assert only supplied fields changed
    - **Validates: Requirements 4.1**

- [ ] 9. GET /deployments route handler
  - [ ] 9.1 Add `listActiveDeployments` handler to `backend/src/routes/deployments.ts`
    - Validate `network` query param; return 422 if absent or invalid
    - Check `getCached(network)`; on hit return cached data with `Cache-Control: max-age=30`
    - On miss: query `deployment_records WHERE is_active = true AND network = $1`, call `setCached`, return with `Cache-Control: max-age=30`
    - No auth required
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5_
  - [ ] 9.2 Write property tests for GET /deployments
    - **Property 12: GET /deployments returns only active records for the requested network** — seed mixed records, assert response contains only `is_active=true` rows for queried network
    - **Property 13: Cache-Control header is present on read responses** — assert `Cache-Control: max-age=30` on every successful GET
    - **Property 14: Cache invalidation ensures post-mutation reads are fresh** — mutate then GET, assert response reflects mutation
    - **Validates: Requirements 6.1, 6.4, 6.5**

- [ ] 10. GET /manifest route handler
  - [ ] 10.1 Create `backend/src/routes/manifest.ts` with `getManifest` handler
    - Validate `network` query param; return 422 if absent or invalid
    - Check cache; on miss query active records for network
    - Shape response as `{ network, contracts: [{ contract_address, semantic_version, wasm_hash, activated_at }] }`
    - Return 200 with empty `contracts` array when no active records exist
    - Set `Content-Type: application/json` and `Cache-Control: max-age=30`
    - _Requirements: 7.1, 7.2, 7.3, 7.4, 7.5_
  - [ ] 10.2 Write property tests for GET /manifest
    - **Property 15: Manifest response shape is correct for all networks** — for each valid network assert shape; assert empty-contracts 200 when no active records
    - **Validates: Requirements 7.1, 7.5**

- [ ] 11. Wire routes into Express app
  - [ ] 11.1 Update `backend/src/index.ts`
    - Register `requireHttps` as global middleware
    - Mount `deployments` router at `/deployments` (with `adminAuth` on POST and PATCH)
    - Mount `manifest` router at `/manifest`
    - Add global error handler that maps `ValidationError` → 422, unknown errors → 500 with `{ "error": "Internal server error" }`
    - _Requirements: 2.4, 3.1, 4.1, 6.1, 7.1_

- [ ] 12. Next.js manifest loader
  - [ ] 12.1 Create `frontend/src/lib/manifestLoader.ts`
    - Implement `loadManifest(network: string): Promise<ContractManifest>` fetching `${NEXT_PUBLIC_REGISTRY_URL}/manifest?network=${network}`
    - Export `ContractManifest` interface matching the manifest response shape
    - Throw on non-200 responses with a descriptive error message
    - _Requirements: 7.1_

- [ ] 13. Seed migration runner with production guard
  - [ ] 13.1 Add `backend/src/db/seed.ts` script entry point
    - Call `migrate.ts` runner; verify `NODE_ENV` guard skips seed file in production
    - Log applied migrations and any skipped files
    - _Requirements: 9.2_
  - [ ] 13.2 Write property test for seed idempotency
    - **Property 16: Seed migration is idempotent** — run seed migration N times against test DB, assert exactly one `local` and one `testnet` row with seed addresses
    - **Validates: Requirements 9.3**

- [ ] 14. Checkpoint — ensure all unit and property tests pass
  - Run `npm test` in `backend/`; ensure all tests pass. Ask the user if any questions arise.

- [ ] 15. Integration tests
  - [ ] 15.1 Write integration tests for DB transaction atomicity (Property 10)
    - Use `testcontainers` or a local Docker Compose PostgreSQL instance
    - Force a mid-transaction error; assert both `deployment_records` and `admin_audit_log` rows are absent
    - _Requirements: 5.4_
  - [ ] 15.2 Write integration tests for cache invalidation (Property 14)
    - POST a record, GET `/deployments`, mutate via PATCH, GET again; assert second response reflects the patch
    - _Requirements: 6.5_
  - [ ] 15.3 Write integration tests for unique constraint (Property 1)
    - POST same `(network, contract_address)` twice; assert second returns 409 and only one row exists in DB
    - _Requirements: 1.2, 3.6_
  - [ ] 15.4 Write integration test for seed idempotency (Property 16)
    - Run seed runner twice; assert row counts remain at 1 per seed address
    - _Requirements: 9.3_
  - [ ] 15.5 Write unit test for audit log append-only enforcement
    - Attempt UPDATE and DELETE on `admin_audit_log` as `app_user`; assert both are rejected by the DB
    - _Requirements: 5.3_

- [ ] 16. Release runbook
  - [ ] 16.1 Create `backend/docs/runbook.md`
    - Document the post-deployment sequence: obtain `contract_address` and `wasm_hash`, call POST `/deployments`, verify 201 response, deactivate previous record via PATCH `{ "is_active": false }`
    - List required environment variables (`ADMIN_API_KEY`, `DATABASE_URL`, `NODE_ENV`) per environment
    - Include example `curl` commands for POST and PATCH with required headers and valid request bodies
    - _Requirements: 10.1, 10.2, 10.3_

- [ ] 17. Final checkpoint — ensure all tests pass
  - Run `npm test` in `backend/`; ensure all unit, property, and integration tests pass. Ask the user if any questions arise.

## Notes

- Tasks marked with `*` are optional and can be skipped for faster MVP (none in this plan — all tests are required per the design)
- Property tests use `fast-check` with `{ numRuns: 100 }` and are tagged with `// Feature: deployment-registry, Property N: ...`
- Integration tests require a PostgreSQL instance; use `testcontainers` or Docker Compose
- The seed migration runner must check `NODE_ENV` before executing `003_seed_dev_data.sql`
- All error responses set `Content-Type: application/json`
