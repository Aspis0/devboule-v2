"""Job output artifact release after a successful run."""

class JobOutputPublisher:
    """Publishes downloadable artifacts when a job reaches terminal status."""

    def __init__(self):
        self.output_renders = []

    def publish_outputs(self, job_id):
        """Create artifact and manifest URLs for a finished job."""
        record = {
            "job_id": job_id,
            "artifact_url": f"/artifacts/{job_id}/result.bin",
            "manifest_url": f"/artifacts/{job_id}/manifest.json",
            "status": "ready",
        }
        self.output_renders.append(record)
        return record

    def download_artifact(self, job_id):
        """Serve a registered artifact as an attachment."""
        return {"content_disposition": "attachment", "job_id": job_id}
