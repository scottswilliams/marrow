#!/usr/bin/env python3
"""Generate a synthetic, referentially consistent EMR export as flat-scalar JSONL.

The output is shaped like a real de-identified EMR export: one JSON object per
line, every member a scalar (string, integer, or boolean), one file per durable
root. It is synthetic test data, not real patient data.

Each file matches a store root's whole payload:
  patients.jsonl           {id, revision, status, display}
  patient_aggregates.jsonl {patient, revision}
  encounters.jsonl         {id, revision, patientId, status, reason}
  observations.jsonl       {id, revision, patientId, encounterId, status, code, value}
  medication_orders.jsonl  {id, revision, patientId, code, status, dose}

Usage: python3 generate.py [PATIENTS] [OUTDIR]   (defaults: 40 patients, .)
"""
import json
import os
import sys

PATIENT_STATUS = ["active", "inactive"]
ENCOUNTER_STATUS = ["planned", "in_progress", "finished", "cancelled"]
OBSERVATION_STATUS = ["preliminary", "final", "entered_in_error"]
MEDICATION_STATUS = ["draft", "active", "completed", "cancelled"]
REASONS = ["A00", "E11.9", "I10", "J06.9", "M54.5", "R51", "Z00.0"]
OBS_CODES = ["GLUCOSE", "HR", "BP-SYS", "BP-DIA", "TEMP", "SPO2", "WEIGHT"]
MED_CODES = ["RX-AMOX", "RX-LISIN", "RX-METFO", "RX-ATORV", "RX-IBUP"]
NAMES = ["Ada", "Grace", "Alan", "Katherine", "Linus", "Barbara", "Dennis",
         "Radia", "Edsger", "Margaret", "Ken", "Frances", "Donald", "Shafi"]


def gen(n_patients, outdir):
    patients, aggregates, encounters, observations, meds = [], [], [], [], []
    enc_id = 1000
    obs_id = 100000
    med_id = 500000
    for pid in range(1, n_patients + 1):
        name = f"{NAMES[pid % len(NAMES)]} P{pid}"
        patients.append({"id": pid, "revision": 1,
                         "status": PATIENT_STATUS[pid % 2], "display": name})
        aggregates.append({"patient": pid, "revision": 1})
        # 1..2 encounters per patient
        pat_encs = []
        for _ in range(1 + (pid % 2)):
            enc_id += 1
            encounters.append({"id": enc_id, "revision": 1, "patientId": pid,
                               "status": ENCOUNTER_STATUS[enc_id % 4],
                               "reason": REASONS[enc_id % len(REASONS)]})
            pat_encs.append(enc_id)
        # 1..3 observations per patient, each under one of the patient's encounters
        for k in range(1 + (pid % 3)):
            obs_id += 1
            eid = pat_encs[k % len(pat_encs)]
            observations.append({"id": obs_id, "revision": 1, "patientId": pid,
                                 "encounterId": eid,
                                 "status": OBSERVATION_STATUS[obs_id % 3],
                                 "code": OBS_CODES[obs_id % len(OBS_CODES)],
                                 "value": 60 + (obs_id % 80)})
        # 0..2 medication orders per patient (distinct codes so no active collision)
        for k in range(pid % 3):
            med_id += 1
            meds.append({"id": med_id, "revision": 1, "patientId": pid,
                         "code": MED_CODES[(pid + k) % len(MED_CODES)],
                         "status": MEDICATION_STATUS[med_id % 4],
                         "dose": 5 + (med_id % 40)})

    files = {
        "patients.jsonl": patients,
        "patient_aggregates.jsonl": aggregates,
        "encounters.jsonl": encounters,
        "observations.jsonl": observations,
        "medication_orders.jsonl": meds,
    }
    for name, rows in files.items():
        with open(os.path.join(outdir, name), "w") as f:
            for row in rows:
                f.write(json.dumps(row) + "\n")
        print(f"{name}: {len(rows)} rows")


if __name__ == "__main__":
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 40
    out = sys.argv[2] if len(sys.argv) > 2 else os.path.dirname(os.path.abspath(__file__))
    gen(n, out)
