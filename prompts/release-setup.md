Add cross-platform release infrastructure to this project.

1. Add blob store upload utilities as a library module with implementations for AWS S3, GCS, and Azure Blob Storage. Expose a common trait/interface with an `upload(local_path, remote_key)` method and a config factory that reads provider selection and credentials from env vars.

2. Create a minimal upload binary that:
   - Takes `--file <path>` and `--key <remote-key>` as CLI args
   - Reads blob store config from env using the library
   - Calls upload and prints the resulting URI
   - Nothing else — all blob logic lives in the library

3. Create a `scripts/release.sh` that:
   - Reads version from the project manifest
   - Cross-compiles for these platforms:
     - linux/amd64
     - linux/arm64
     - darwin/amd64
     - darwin/arm64
     - windows/amd64
   - Names each artifact: `{bin}-v{version}-{os}-{arch}[.exe]`
   - Uploads each to `releases/v{version}/{artifact}` and `releases/latest/{bin}-latest-{os}-{arch}[.exe]` using the upload binary

Env vars for blob store config:
- BLOB_PROVIDER: s3 | gcs | azure
- S3: S3_BUCKET, AWS_REGION
- GCS: GCS_BUCKET, GCS_ACCESS_TOKEN or GOOGLE_APPLICATION_CREDENTIALS
- Azure: AZURE_STORAGE_ACCOUNT, AZURE_STORAGE_CONTAINER, AZURE_STORAGE_ACCESS_KEY
