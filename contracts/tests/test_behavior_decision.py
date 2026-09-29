import copy
import json
import unittest
from datetime import datetime
from pathlib import Path

from jsonschema import Draft202012Validator, FormatChecker


ROOT = Path(__file__).resolve().parents[1] / "behavior" / "v1"


class BehaviorDecisionContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        schema = json.loads((ROOT / "decision.schema.json").read_text(encoding="utf-8"))
        Draft202012Validator.check_schema(schema)
        cls.validator = Draft202012Validator(schema, format_checker=FormatChecker())
        cls.valid = json.loads(
            (ROOT / "examples" / "decision-apply.json").read_text(encoding="utf-8")
        )

    def test_example_and_required_fields(self):
        self.validator.validate(self.valid)
        for field in ("decision_id", "household_id", "subject_id", "device_id",
                      "capability_id", "valid_from", "expires_at", "reason_code",
                      "idempotency_key", "required_confirmation"):
            with self.subTest(field=field):
                invalid = copy.deepcopy(self.valid)
                del invalid[field]
                self.assertFalse(self.validator.is_valid(invalid))

    def test_invalid_version_time_and_confirmation(self):
        for field, value in (
            ("schema_version", "2.0"),
            ("expires_at", "2026-09-29T21:00:00+02:00"),
            ("required_confirmation", "accepted"),
            ("issuer", "mobile-client"),
        ):
            with self.subTest(field=field):
                invalid = copy.deepcopy(self.valid)
                invalid[field] = value
                self.assertFalse(self.validator.is_valid(invalid))

    def test_time_window_order_is_semantic_rule(self):
        start = datetime.fromisoformat(self.valid["valid_from"].replace("Z", "+00:00"))
        end = datetime.fromisoformat(self.valid["expires_at"].replace("Z", "+00:00"))
        self.assertLess(start, end)


if __name__ == "__main__":
    unittest.main()
