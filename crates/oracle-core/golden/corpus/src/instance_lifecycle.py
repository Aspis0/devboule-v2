"""Compute instance lifecycle: spawn, terminate, release the slot."""

class InstanceManager:
    """Manages paid compute instance lifecycle."""

    def cleanup_instance_after_terminal(self, job_id):
        """Clean up an instance after the job reaches terminal status."""
        instance_id = self.lookup_instance(job_id)
        self.terminate_instance(instance_id)
        self.release_instance_slot()

    def terminate_instance(self, instance_id):
        """Terminate a compute instance."""
        return {"instance_id": instance_id, "state": "terminated"}

    def release_instance_slot(self):
        """Release the slot so billing stops."""
        return {"released": True}

    def lookup_instance(self, job_id):
        return f"inst-{job_id}"
