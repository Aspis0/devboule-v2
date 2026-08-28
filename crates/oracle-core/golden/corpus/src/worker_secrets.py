"""Worker secret rotation and management."""

class WorkerSecretManager:
    """Manages worker secret lifecycle."""

    def __init__(self):
        self.secrets = {}

    def rotate_worker_secret(self, worker_name, secret_name, new_value):
        """Rotate a worker secret."""
        validation = self.validate_secret_rotation_request(worker_name, secret_name)
        if not validation["ok"]:
            return validation
        return self.put_worker_secret(worker_name, secret_name, new_value)

    def put_worker_secret(self, worker_name, secret_name, value):
        """Write a secret value to a worker."""
        self.secrets[(worker_name, secret_name)] = value
        return {"written": True}

    def validate_secret_rotation_request(self, worker_name, secret_name):
        """Validate a rotation request."""
        return {"ok": bool(worker_name and secret_name)}

    def list_worker_secrets(self, worker_name):
        return [name for (w, name) in self.secrets if w == worker_name]
