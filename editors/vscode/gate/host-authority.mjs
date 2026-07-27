#!/Users/scottwilliams/.nvm/versions/node/v24.16.0/bin/node

import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  chownSync,
  closeSync,
  constants as fsConstants,
  fstatSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  rmdirSync,
  unlinkSync,
  writeSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { Readable } from "node:stream";
import { fileURLToPath } from "node:url";
import { Socket } from "node:net";
import { types as utilTypes } from "node:util";
import { gunzipSync } from "node:zlib";

const OWNER_PATH = fileURLToPath(import.meta.url);
const NODE =
  "/Users/scottwilliams/.nvm/versions/node/v24.16.0/bin/node";
const MKFIFO = "/usr/bin/mkfifo";
const SANDBOX_EXEC = "/usr/bin/sandbox-exec";
const PS = "/bin/ps";
const RUBY = "/usr/bin/ruby";
const RUBY_FRAMEWORK =
  "/System/Library/Frameworks/Ruby.framework/Versions/2.6/usr/bin/ruby";
const RUBY_BASE =
  "/System/Library/Frameworks/Ruby.framework/Versions/2.6/usr/lib/ruby/2.6.0";
const RUBY_PLATFORM = `${RUBY_BASE}/universal-darwin25`;
const RUBY_ENCDB = `${RUBY_PLATFORM}/enc/encdb.bundle`;
const RUBY_TRANSDB = `${RUBY_PLATFORM}/enc/trans/transdb.bundle`;
const RUBY_SOCKET_BUNDLE = `${RUBY_PLATFORM}/socket.bundle`;
const RUBY_IO_WAIT = `${RUBY_PLATFORM}/io/wait.bundle`;
const RUBY_SOCKET_RB = `${RUBY_BASE}/socket.rb`;
const RUBY_DYLIB_LINK =
  "/System/Library/Frameworks/Ruby.framework/Versions/2.6/usr/lib/libruby.2.6.dylib";
const DYLD = "/usr/lib/dyld";
const DYLD_CACHE =
  "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e";
const DYLD_CACHE_MAP = `${DYLD_CACHE}.map`;
const DYLD_CACHE_ATLAS = `${DYLD_CACHE}.atlas`;
const SYSTEM_VERSION =
  "/System/Library/CoreServices/SystemVersion.plist";
const DEV_NULL = "/dev/null";

const HOST_UID = 501;
const HOST_GID = 20;
const MAX_U64 = 18_446_744_073_709_551_615n;
const STREAM_BUFFER_BYTES = 65_536;
const OWNER_MAX_BYTES = 1_048_576;
const EVIDENCE_MAX_BYTES = 875_604;
const RESULT_MAX_BYTES = 4_096;
const PID_MIN = 2;
const PID_MAX = 99_999;
const TOMBSTONE_BYTES = 12_500;
const CANARY_BYTES = Buffer.from("VSQ01-A0-AMBIENT-CANARY-V1\n");
let activeS2WorkerRelayStream;
let activeS2InvocationState;

const PACKET = Object.freeze({
  version: "VSQ01S1_PHASE_A0_MAIN_V2",
  design: Object.freeze({
    bytes: 4_451,
    lines: 138,
    sha256:
      "e4ac66024783c94a7b480ca6a308a98511cfcf9b3a237ed9df9c39f858a22a26",
  }),
  checker: Object.freeze({
    bytes: 28_805,
    lines: 979,
    sha256:
      "99b26ebd6bc74dbd204ec1e69c7f710d1fa88abb8f7d36df684bf2198406149a",
  }),
  manifestSha256:
    "a9e68dc484591ae9939c73056b595bc502fd36ed640d351f4f7fb8d71b1645c3",
  pathNul: Object.freeze({
    bytes: 42,
    sha256:
      "0f1adf61cf59eec4de1b9cfe9089016ae5fc8a2887132231a2b11ce34e7baaee",
  }),
});

const FIXED_PINS = Object.freeze([
  Object.freeze({
    role: "node",
    path: NODE,
    bytes: 120_573_328,
    uid: 501,
    gid: 20,
    mode: 0o755,
    sha256:
      "1ee75375e33b94fc34b3b19aede049e11dae90efb63b374dc96d6bdace70c4b8",
  }),
  Object.freeze({
    role: "mkfifo",
    path: MKFIFO,
    bytes: 101_344,
    uid: 0,
    gid: 0,
    mode: 0o755,
    sha256:
      "a193c702dc4c53eddd7ba404775a4322a602f3a3143eaf6b4b183359fc760414",
  }),
  Object.freeze({
    role: "sandbox-exec",
    path: SANDBOX_EXEC,
    bytes: 102_560,
    uid: 0,
    gid: 0,
    mode: 0o755,
    sha256:
      "8290e4be7387a0df83cd1559e86afd880464f269450573d012795761fe298f16",
  }),
  Object.freeze({
    role: "ps",
    path: PS,
    bytes: 170_816,
    uid: 0,
    gid: 0,
    mode: 0o4755,
    sha256:
      "472992c470606d28f577590decfecd7f4a20f832fd92c671bebc6d44790b5d02",
  }),
  Object.freeze({
    role: "ruby",
    path: RUBY,
    bytes: 135_200,
    uid: 0,
    gid: 0,
    mode: 0o555,
    sha256:
      "9d6ff3e289c7d908e3c785e0bedd6692d1d6a3377965c88c04d847104b7c892c",
  }),
  Object.freeze({
    role: "ruby-framework",
    path: RUBY_FRAMEWORK,
    bytes: 135_200,
    uid: 0,
    gid: 0,
    mode: 0o755,
    sha256:
      "b23faefc09375bbc24248218271e30d25590168e1a1f977bdfc528ce547b2c81",
  }),
  Object.freeze({
    role: "ruby-encdb",
    path: RUBY_ENCDB,
    bytes: 117_504,
    uid: 0,
    gid: 0,
    mode: 0o755,
    sha256:
      "de95c352f6ff9788452c890ee7889e6d8912830a88d8b681eb2f371fad0e4510",
  }),
  Object.freeze({
    role: "ruby-transdb",
    path: RUBY_TRANSDB,
    bytes: 117_136,
    uid: 0,
    gid: 0,
    mode: 0o755,
    sha256:
      "8d8ed5a500287b21d9652cb3b44fe67dffe2352993adb10320743ca68557e87f",
  }),
  Object.freeze({
    role: "ruby-socket-bundle",
    path: RUBY_SOCKET_BUNDLE,
    bytes: 399_200,
    uid: 0,
    gid: 0,
    mode: 0o755,
    sha256:
      "87c1b348cf3082e6f16ae37b85656e529c05447d57cc3f52f8b022e2636d9ddb",
  }),
  Object.freeze({
    role: "ruby-io-wait",
    path: RUBY_IO_WAIT,
    bytes: 101_632,
    uid: 0,
    gid: 0,
    mode: 0o755,
    sha256:
      "fb1f1cfe4ba0ee9c5da9f70bd715ea6bdc9a386cbb713bfbb68dfe1b9f0f631f",
  }),
  Object.freeze({
    role: "ruby-socket-rb",
    path: RUBY_SOCKET_RB,
    bytes: 44_554,
    uid: 0,
    gid: 0,
    mode: 0o644,
    sha256:
      "72fc396171abc06ece8955b8529271d6a8753b8d87ad61b0cc24bc946f29f3f9",
  }),
  Object.freeze({
    role: "dyld",
    path: DYLD,
    bytes: 2_374_000,
    uid: 0,
    gid: 0,
    mode: 0o755,
    sha256:
      "6da2d109f72330d031450f3c0ebea14bfc10f42f844a958858e16a4092c38f12",
  }),
  Object.freeze({
    role: "dyld-cache-header",
    path: DYLD_CACHE,
    bytes: 573_440,
    uid: 0,
    gid: 80,
    mode: 0o755,
    sha256:
      "90161f0fc880cd6e54043a603ab44482cd187aff10c44d004edf21ac4d9aace4",
  }),
  Object.freeze({
    role: "dyld-cache-map",
    path: DYLD_CACHE_MAP,
    bytes: 1_336_598,
    uid: 0,
    gid: 80,
    mode: 0o755,
    sha256:
      "2a8103af20aa9c83d27ba1d09931e6677ce5335d27d2197cb5b1a1656b9a6d07",
  }),
  Object.freeze({
    role: "dyld-cache-atlas",
    path: DYLD_CACHE_ATLAS,
    bytes: 2_369_991,
    uid: 0,
    gid: 80,
    mode: 0o755,
    sha256:
      "46cb6eb9a1f30e22f1333343c45621ce230639e37456582386ad0bc1ac5f71e1",
  }),
  Object.freeze({
    role: "system-version",
    path: SYSTEM_VERSION,
    bytes: 603,
    uid: 0,
    gid: 0,
    mode: 0o444,
    sha256:
      "cbf534776ca9200252e5637787e5d4fc26cf527fb354c19bcbac9688341c7a58",
  }),
]);

const PARTITION_CAPS = Object.freeze({
  static: 9_728,
  fifo_facts: 11_264,
  fifo_batches: 25_856,
  ps: 90_112,
  node: 9_216,
  ruby: 114_688,
  supervisor: 131_072,
  relay: 110_804,
  transitions: 98_304,
  tombstone: 32_768,
  roots: 12_416,
  closeout: 65_536,
  envelope: 163_840,
});

const LIMITS = Object.freeze({
  proofExecutedMax: 36,
  captureAttempts: 37,
  psCaptures: 37,
  fifoBatches: 43,
  fifoInodes: 90,
  fifoPathBytes: 6_574,
  fifoLongestPath: 74,
  fifoArgvBytes: 246,
  taskFifoDescriptors: 17,
  protocolDescriptors: 59,
  directories: 70,
  regularFiles: 2,
  sockets: 7,
  nodeLegs: 2,
  rubyLegs: 4,
  rubyCustodySupervisors: 1,
  protocolSocketpairs: 3,
  protocolEndpoints: 6,
  startupPipes: 8,
  startupPipeEndpoints: 4,
  evidenceRecords: 124,
});

const CAPACITY_MAXIMA = Object.freeze({
  directories: LIMITS.directories,
  regularFiles: LIMITS.regularFiles,
  fifoBatches: LIMITS.fifoBatches,
  fifoInodes: LIMITS.fifoInodes,
  fifoPathBytes: LIMITS.fifoPathBytes,
  captureAttempts: LIMITS.captureAttempts,
  psCaptures: LIMITS.psCaptures,
  proofs: LIMITS.proofExecutedMax,
  nodeLegs: LIMITS.nodeLegs,
  rubyLegs: LIMITS.rubyLegs,
  sockets: LIMITS.sockets,
  rubyCustodySupervisors: LIMITS.rubyCustodySupervisors,
  protocolSocketpairs: LIMITS.protocolSocketpairs,
  protocolEndpoints: LIMITS.protocolEndpoints,
  startupPipes: LIMITS.startupPipes,
  startupPipeEndpoints: LIMITS.startupPipeEndpoints,
  descriptorSlots: LIMITS.protocolDescriptors,
});

const DEADLINE_MS = Object.freeze({
  outer: 660_000,
  initialStatic: 120_000,
  supervisorSetup: 10_000,
  preSpawnRecheck: 60_000,
  worker: 420_000,
  workerEntry: 10_000,
  fifoBatch: 500,
  ps: 3_000,
  node: 12_000,
  ruby: 50_000,
  finalStatic: 120_000,
  artifactCloseout: 10_000,
  supervisorPostWorker: 10_000,
  workerForced: 15_000,
  supervisorBootstrap: 10_000,
  supervisorCommand: 10_000,
  relay: 10_000,
});

const S2_FAULT_CLOSEOUT_MS = Object.freeze({
  reserve: 30_000,
  workerNormal: 1,
  workerTerm: 3_000,
  workerKill: 3_000,
  supervisorNormal: 8_000,
  supervisorTerm: 4_000,
  supervisorKill: 4_000,
});

function requireS2FaultCloseoutReserve(reserveMs) {
  const serialMs =
    S2_FAULT_CLOSEOUT_MS.workerNormal +
    S2_FAULT_CLOSEOUT_MS.workerTerm +
    S2_FAULT_CLOSEOUT_MS.workerKill +
    S2_FAULT_CLOSEOUT_MS.supervisorNormal +
    S2_FAULT_CLOSEOUT_MS.supervisorTerm +
    S2_FAULT_CLOSEOUT_MS.supervisorKill;
  requireCondition(
    Number.isSafeInteger(reserveMs) &&
      reserveMs > serialMs,
    "host.s2_fault_closeout",
    "fault closeout reserve lacks strict post-settle overhead",
    { reserveMs, serialMs },
  );
  return Object.freeze({ reserveMs, serialMs });
}

const NODE_LITERAL_BASE64 =
  "H4sIAAAAAAACE7VVbW/bNhD+KzERGCRKy3biuXEcOlhTowiwJUG8fJntrrJ0cojKpEJS9jxL/30g9RJrKdZhQD/pdDry7p7nuVMghTYnkWYKXlKuACMhQ7iMNCJUgPmHW4BBZBy4MwlLlAxAa89X6+28v6RPwwHDfXF1NRwI0umLMjCE7X3ENJscynemvRC2Yx7hD3x9K4zn61th7vBwQEPSYiwkCkyqxIlI47i8JGV17BOvg8dlYDrpiev00sbnlAtZ5NMeF3LSE+22s67Y03Bw7czLo5v9lQZhGCZscjBqf4i0F2vjm9leBDihhxVfc2EujUohrxNGfqwhD3wTPGMgh9IL7TZ4gQyBMYamd/fTu99QnttGj6HyYhBr89xi7CzLWt3Pi26i+NY3sOiaTbLobnyl5K6z1S8dvzP3O1GvM1oeLvKFN/+587vf+cu9D3N7DqKYr5/Nomvp6WijwN8suoEURsl44WkZfD3tpp4BbXBCsuxDGkWgvNXewC+uCpxYxN8Ps6xVAIEJOVTVwp/c4IseyWMwJ3InIGSuccpDmigIYqmhRNF9vQ1BGG72BZg8wi3nzrIWb3JqcS7OafaveNOQOflgTShnjlmsiUW0pT2uZzL4CgaTLNNeysMWK1VS470Gk/IQE5JlIWPMJs8yXlv2BA/nvWWW8cLsLxuFlvY8pHxZ0H04+pznlcQ5MCwolG0DKbvbMLNPQEYncF3rQhvFxRq1293Pls9eZ/TH8tCn52d5zVQhInJdPN+hhUCXaPr4eP+4EIiuWMlipOQGbxwakfZ2ihtwIJ7RlSV1VQqNCDY6yxucClJVrkFtQTEBxgsU+AZmzoG1Q5ZNDpb6OqMfxzLAPTIuPntSYBT6xkc0KDqvcr4LSmMyICEHfDGgKPH3sfRDRMb1fYEUgW/wfEWDJX1z1nVWeRljA+I0tfLgJfVjjY9hQA+3d58QIUW2nyhKLMh1oSBCjB7ubUieN8oHpaRCFNjEnRxSIMRGOBg8KQI4jnHsNqb8Yfr4K2q3X4enwFU1SPrycXp3O/2YndajlfAwP3517+6yhfjyltI+VZZSVSHTYHN0TsYNR4/krpk+haNeYq4NCJxQN52WV+0G8Tsj+Lrl3J1n9s5qkf+42Sx4PKeIlzsFkXGxgWxVYx4yN5OlioOY2x3+quIbKQQEhkuBEzL+toaLQ04EQRGOHDSl34Ff6+o4+r8p/v0PV/z9keIvKEqkU3xZqFV89avescZ6xgVJu6aKLkZWRcVSZ7tCGYVwnAtX6s+yVhWWZZU17y1fV2nt7C+rpXr8e3mT9ZsTM3u6uZnOZt8bmdODS1oa/WX+v8anUnnzzzcakNxuiyPqm9ti1Cu3xd8rWdLCRgkAAA==";

const RUBY_LITERAL_BASE64 =
  "H4sIAAAAAAAAA71Y/W/aOBj+V4J3mkAKgXa7612RN1VAq+o2QNBOk7gKhcQFq6kT2UkpV/q/n7+S2CHctHXdD4X4zevn9fv5mKLHBAUpChe3yE8zihicA0Sye0T9NKYei4EL0jVFfujRJX/mYhwTP1Jvgvg+idCjWnRmW5ai+84nvKQ+3XbOqX+PNjG9Y51pttx6t/m68wVRxlFY59j7o5Mx2onwskO5jhB43U5G8ANX8aN26NMNJse/dxAJxF+49JYZCSP0K+yl1Od75Ocvs8vi4A6lv8gYjjsbH7+KNe0Hr5mbnkN9zJAD8hIDTkYixJjz26fx2WA4WJwPz66up8MZhKhajsVmn64eio1n04svXoTIKl1DeNxzEp8iki4SP127wRpHoXyEQq0AEBJuGt+a2hCW6j1nXo9z4yE/WDth7OzEctdzlmiFSc85xxHyIpb6aVO8aFm22ugRs5QBLkQsyJAzpJTEp6fD0Xg4uuo5iIT6I4iQT7JEHaj9QWK58Yag0MVh6ym3xlK4b5C705Cqzm7n4NAjOPooHhss9VQO5JKvMhw24ITGAQ+gWAjxnMtD9ODyL0zimwbE/Di3fsQQP5r8lBZ51DG5y00edF5vrHc3pVn5bpb6hFdhyHViWlosIlIPoRz9NojzLHQChJN0gWN4OfYI2jTfuWADWuYbj21JANXB1nEUitjztzEVQdeZ1NpF5Vm72YbiFOV7IFQP3nKbIob/RZatIIrF8fIE3OEoaoLZ1XgC3FyW8Gyrs/vhgvvhSnjxJHxIcML3i35FIdTO6hrFXMCbkldouTk3qFRUKUBeHblExtLG4TCGAs/sCkHAQweKnFtgM9XiIrbq8fT07HxxPbr86ubr2bj/92JwMT377HZbFeAlJiGwIT0h02Cyev0wpIuMNMtebFVh+EzDt1tgSK02MXYW3SllbSEr0lpuLrrm7VtTyhsGmt1jxGxuqIlmMpaiqeyAq2qzPLj36R2ioFeWnB9ui5MVNeCpYgNTwCvtqGe8sDOtQEU9A1XWXuCLWuvvZrs3T0atPZvLynqF0mRFk4qI4bDZbQlh7vy8e2Mtj26e/yGgmiPeBg+lfwkmq2rgNVtwveZ77h6YXI4uQAWFIVkwOUp8CEXoNcFkzBHcrltTTcaUb3Fj7/nQeMSp0y0ni2TBPg+bnC3wA5KD1jyNrl9RJchDYk5BmI+ryXD6mX+JVdE5e2kcHIkw7SWxfrbJ8538qaejldTBz0qq8Gd3pLKn7J3kJHDIh6955mVMnk/fPOlYPKtBW+uKHPKWP4gwTvV5tjUb6k7RpquDtGYcS3n48ZBdG9xknJrZpo2UOZdSkezGvtoLbFbSv9/zP2yg5k3RLMZ1Q5VROSRdY1a5eVfrgmg4J3/ZDaeRTObeK2jdbCYBaZHFQFqWU1C+q4aDbMDvJyEbWrOQBXqQhqzBUUUqiKgUVy5s5WbjoiiENhcZ+00yMsX7bFTGb24qCj4y14qQrASoPrN92WePyc9kj+Kwmj7KdcEf9nkUY1eJUkltuiyuP+KheSS4ZAqqcBUm4XzUlqJqBiw+uTzIJ+btRNKJba3Cfpy32lJUb83gwLHkwIVxochYkXVxETzOTcse1VdDfdE2Q6S21lx2MuaxLBB4fMQkWcocMLvu94ezGfhfMlToe/X6Pbxo1darkJgqF2hVxMvCqR3WwFBwuH1N5PEUk1JjQ8GgKqqD4ehyOAAl1+WTXhqp/izoj0dXwC0Pk6dCjkZb9YrH9JuqwkrzgI7lphzhOp+vxMMV9BpStPuhYGJLLKm4RvEldu1fTnsD5cfx614dJmPzfxDmoHaLKWn8klG7q6PE5OX/AFDvOKzhEwAA";

const NODE_LITERAL = gunzipSync(
  Buffer.from(NODE_LITERAL_BASE64, "base64"),
).toString("ascii");
const RUBY_LITERAL = gunzipSync(
  Buffer.from(RUBY_LITERAL_BASE64, "base64"),
).toString("ascii");

const S2_TARGET_LITERAL_BASE64 =
  "H4sIAAAAAAACE71YfW/bNhP/Kgr3YJAeyLKdtetqjQ2CxC2CtXZgp0MB1xBk8WITViiDpOJ4Tb77A4p6IWVle7au/cOGeOTd8V5/J/GYCnBQzNf3yMlZCkI457N3vwcpsLXcYPwi3MUcmIx2sdz4yYamRD9mO+CxpBmLUlj7WS6BRwRiklIGkYQHiZWckGsF1U6tpIMhuItlsjlz+5/PF8Pe6+Vi0Hu9/O/nP/pe2DrNBO7il1lEQyFjLvNdxGGXcYmvpgGDvfvCR3vktTYDcWAJljwHYyOFWEDF9tJHHHkhPEAScUgyTjAafxpfRNPfPjNUGae2e1pkbd+RJrHnVIJriPIwNlbB6iBB0D8gLO8QFQSMULAK0yzbOSQLWbZXxl/zLAEhgiTNkm20BinpHbgldTS6eD+9+C36MJ1Mb6aTqwt/xGKWCUgyRrzqzqWSXhMYeuto8W+OvB1yiMlBOUVACol0Fy1/LX1G0+K3cI+4e1qs1x9Gg8Gg+gUDfxAMXi6DO8q8kMGDVDcoFAWMpmdhssnZFrcUBepAxDK2Uqa7w9PXPctdtRd9eEhgp/JzdBunAhoVWi4e7WMqIyUuXqUQrjjE23pb38CS/OuvxU7bf6ssZwTpq3fd483w9JcQGGnx1XliBxuj2fj9+Hw+LhOsbX2SZqJOEFKnZMl0aadldez/Tc2WWA/jFqVJ0ZYIfS142EEigUS3EMucg8ALBCy/U30i44HIkI/kRnk84CvkI90+4lTvJNndLoUHvejPD0LCXf89XfGYH/pveXwH+4xvRX+Wrw7BbbXu/w5c0IyJ/mnwcz8XvJ/SVZ/nq4MiBIN+zug9cBGnPRLzPWWnL/vAEvUjq2CVM5LC99AnecyE/v9uekWWbEF+J2U066uC+hbaSjv4Ci1Dp8ztKsXqpP7P++n55fgyejs+v/k4G89Vb22lY+hU3ArARFG0Brhh3KBb6Cy6YW8ZQJxsHJI5j2r5GDorWFMWOm9pCkEqZCxdteGFpq4ePFAhBQodDiLJwRlzzrLRaDyZjic3oaP6g/5LUohZvtMX6r1xNdTuGRCfEu9LpU1IfKyQ3jonxVHn8dGhpOhh6vFEyED7sFgKGeSUnNQgktOCYSFkQODeFzKgLFueYEpCp2idoQPFf6ExZyll20rls8aXjN3mFohb7c1lzEjMyZjzjDcaa490i9CG/rUQ50mdSYDuZESzCtZ/0tOAsWOMAs4mS4nyfdkGvyjHWtPOCUYZg17JjZT3ygZZ9MVoT+XmzEXXj6hJg/pw3fcN1XUDrvuu3W4rT9QsuuE6VQi3NE1dNL+ZXiO/ou0o8bT1MYmAEb/QoZ6UF3Z0B6GjKhYILt1VZjkl+DbjWzVu1MyVQn1EJxNmNK0oRTRsOZSYB4SM14ARgz2qs8YSNtdFrqKjH0ej87fRx8nVJ79az9VUc/ludv7BH3gtwSvKCLJFBopWCivyPyaERzlzm2r22mLugdPbAzKoVqEZnHVgC1pP0erYNsx13f34o0nNKcFm/Rk+WxjHVDkaS1WWtsN1vloW3MV8CxwZeReTQ32zOgcCnXFohjyMh6GxYUdaC1UVgXRhBEmscu3icf74wxcj157MZWu9Brlb812LJChxB54iVsYvBktrOVw+fWaoHSMOyX1j346yddvxJV5Acu++8DBG11eTd6glRUCRMJWU7Dkp6pyLrqeTd8gf+B3ZZOCE56n3JQceqHQGTW8qcPAiTtOiO+E3EOo5s7lNmb8qSyAA1ekwrhre9Xj2YTQqVnXlHIXxcqjcdBTE7u5Y3O/VL2VXsYJ6+W8FVdnzONTR0/peVTDynA2fqsgXPnka/fCl9MWTbtWdphQwYdkDTOS8TuAST8tKKVW3G2lHTy7o5Ow5vbZwE7M6eluppIl5QVXBPjk+9hU6W+E/rvl/rKBjpy4WY2DRaWR8IzB6lV9VdZkQJ86r13bBlZJM7D9K6LLYTAAqSRYClbQKgiquDgyyBf59ELJFlyhkCX0WhqzG0ZZUA1FDbo18DbMxaiqijUUGvwlGJvkYjRr/LcyDCo/MtQYkKwC6zmxbjtHj+t9Ej/qyJXw06xo/7PtoxG4DpabacFmPP+rBHSosmaG2uBaSULbuFaR2BCw8uXoWT8zppIATW1sL/TK27hWkbm0GBk4LDIyMgSJvPiSpQfDUrQdArx4Ny1HddJFm7Rh2chGIPFHyzkJnl0vhoPnHi4vxfI7+FAy19KN8/Tu4aOXWNwExnS7Yyoivc2dpcCkYKwy3x8RcBKpTlrKxQlDt1cvx5Gp8iRqsqzp9oaT9WnAxndwgv7lMFYqiNdpHb8azD395VGlxnzljmVm08DKe3wiHW9I7QNGuhxqJLXIBxR0Hv0av/eZ01FD+ufyurefB2PyKYTZqv+6SxpuM5m63EhOX/weI9nVCLxgAAA==";
const S2_TARGET_LITERAL = gunzipSync(
  Buffer.from(S2_TARGET_LITERAL_BASE64, "base64"),
).toString("ascii");

const S2_LEGS = Object.freeze([
  "support",
  "denial",
  "one-receipt",
  "parent-loss",
]);

const S2_TRANSITION_PLAN = Object.freeze({
  support: Object.freeze(["GROUP_CONT"]),
  denial: Object.freeze(["GROUP_CONT"]),
  "one-receipt": Object.freeze([
    "LEADER_KILL",
    "SELECTED_SUFFIX_ALREADY_STOPPED",
    "GROUP_CONT",
    "GROUP_TERM",
  ]),
  "parent-loss": Object.freeze([
    "LEADER_KILL",
    "SELECTED_SUFFIX_ALREADY_STOPPED",
    "GROUP_CONT",
    "GROUP_TERM",
  ]),
});

const S2_PROOF_ORDINALS = Object.freeze({
  support: Object.freeze([1, 2, 3, 13, 14]),
  denial: Object.freeze([1, 2, 3, 13, 14]),
  "one-receipt": Object.freeze([
    1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,
  ]),
  "parent-loss": Object.freeze([
    1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,
  ]),
});

const S2_PROTOCOL = Object.freeze({
  supervisorInputFrames: 27,
  supervisorOutputFrames: 28,
  supervisorFaultOutputFrames: 29,
  relayFrames: 28,
  relayEnvelopes: 28,
  frameBytes: 1_024,
  resultEnvelopeBytes: 4_096,
  supervisorInputBytes: 27_648,
  supervisorOutputBytes: 28_672,
  relayFrameBytes: 28_672,
  relayEnvelopeBytes: 114_688,
  startupRecordBytes: 128,
  startupReportBytes: 384,
  startupReleaseBytes: 128,
  cleanEvidenceRecords: 121,
  replacementEvidenceRecords: 123,
  retainedFaultEvidenceRecords: 124,
});

const S2_PACKET = Object.freeze({
  version: "VSQ01S2_PHASE_A0_MAIN_V1",
  scheduler: "5099e31902395ea10d1cca2ee061fc8f3904c748",
  laneSha256:
    "74444d6688e77a75cbb3c0bf52ba4fb854e15b220554cacd2062191ae1970487",
});

const CUSTODY_SUPERVISOR_LITERAL = String.raw`
STDOUT.sync=true
class CustodyFault < StandardError
  attr_reader :code,:details
  def initialize(code,message,details="".b)
    @code=code
    @details=details
    super(message)
  end
end
def stop!(code,message,details="".b)
  raise CustodyFault.new(code,message,details)
end
def canonical_decimal(value)
  value=="0" || value.match?(/\A[1-9][0-9]*\z/)
end
def rotr(value,bits)
  ((value>>bits)|(value<<(32-bits)))&0xffffffff
end
def sha256(bytes)
  mask=0xffffffff
  constants=[
    0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
    0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
    0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
    0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
    0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
    0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
    0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
    0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2
  ]
  state=[0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19]
  data=bytes.b.bytes
  bit_length=data.length*8
  data << 0x80
  data << 0 while (data.length % 64) != 56
  8.times { |index| data << ((bit_length >> (56-index*8)) & 0xff) }
  data.each_slice(64) do |block|
    words=Array.new(64,0)
    16.times do |index|
      offset=index*4
      words[index]=((block[offset]<<24)|(block[offset+1]<<16)|(block[offset+2]<<8)|block[offset+3])&mask
    end
    (16...64).each do |index|
      x=words[index-15]
      y=words[index-2]
      s0=(rotr(x,7)^rotr(x,18)^(x>>3))&mask
      s1=(rotr(y,17)^rotr(y,19)^(y>>10))&mask
      words[index]=(words[index-16]+s0+words[index-7]+s1)&mask
    end
    a,b,c,d,e,f,g,h=state
    64.times do |index|
      sum1=(rotr(e,6)^rotr(e,11)^rotr(e,25))&mask
      choice=((e&f)^((~e)&g))&mask
      temp1=(h+sum1+choice+constants[index]+words[index])&mask
      sum0=(rotr(a,2)^rotr(a,13)^rotr(a,22))&mask
      majority=((a&b)^(a&c)^(b&c))&mask
      temp2=(sum0+majority)&mask
      h=g;g=f;f=e;e=(d+temp1)&mask;d=c;c=b;b=a;a=(temp1+temp2)&mask
    end
    state=[state[0]+a,state[1]+b,state[2]+c,state[3]+d,state[4]+e,state[5]+f,state[6]+g,state[7]+h].map { |value| value&mask }
  end
  state.pack("N8").unpack1("H*")
end
def monotonic_ns
  Process.clock_gettime(Process::CLOCK_MONOTONIC,:nanosecond)
end
def require_deadline(deadline)
  stop!("DEADLINE_STOP","outer deadline expired") if monotonic_ns>deadline
end
def ascii_frame?(bytes)
  bytes.bytes.all? { |byte| byte==10 || (byte>=32 && byte<=126) }
end
def b64(bytes)
  [bytes].pack("m0")
end
def unb64(value)
  decoded=value.unpack1("m0")
  stop!("PROTOCOL_STOP","noncanonical base64") unless b64(decoded)==value
  decoded
end
class BoundedReader
  attr_reader :total,:retained,:eof
  def initialize(io,deadline,record_max,total_max)
    @io=io
    @deadline=deadline
    @record_max=record_max
    @total_max=total_max
    @pending="".b
    @retained="".b
    @total=0
    @eof=false
  end
  def read_record
    loop do
      newline=@pending.index("\n")
      unless newline.nil?
        length=newline+1
        stop!("PROTOCOL_STOP","record bound") if length>@record_max
        record=@pending.byteslice(0,length)
        @pending=@pending.byteslice(length,@pending.bytesize-length) || "".b
        return record
      end
      if @eof
        stop!("PROTOCOL_STOP","partial record EOF") unless @pending.empty?
        return nil
      end
      require_deadline(@deadline)
      remaining=@deadline-monotonic_ns
      stop!("DEADLINE_STOP","read deadline") if remaining<0
      ready=IO.select([@io],nil,nil,[remaining/1_000_000_000.0,0.05].min)
      next if ready.nil?
      available=@total_max-@total+1
      chunk=@io.read_nonblock([available,1024].min,exception:false)
      next if chunk==:wait_readable
      if chunk.nil?
        @eof=true
        next
      end
      require_deadline(@deadline)
      stop!("PROTOCOL_STOP","empty read") if chunk.empty?
      stop!("PROTOCOL_STOP","high-bit input") unless ascii_frame?(chunk)
      @total+=chunk.bytesize
      stop!("PROTOCOL_STOP","aggregate input bound") if @total>@total_max
      @retained<<chunk
      @pending<<chunk
      stop!("PROTOCOL_STOP","record bound") if @pending.bytesize>@record_max && !@pending.include?("\n")
    end
  end
  def require_eof
    stop!("PROTOCOL_STOP","trailing record") unless read_record.nil?
  end
end
class FrameOwner
  attr_reader :input_count,:output_count,:input_bytes,:output_bytes
  def initialize(deadline)
    @deadline=deadline
    @reader=BoundedReader.new(STDIN,deadline,1024,27648)
    @input_count=0
    @output_count=0
    @input_bytes=0
    @output_bytes=0
  end
  def read
    line=@reader.read_record
    return nil if line.nil?
    stop!("PROTOCOL_STOP","input frame bound") unless line.bytesize<=1024 && line.end_with?("\n") && ascii_frame?(line)
    stop!("PROTOCOL_STOP","input aggregate bound") unless @input_count<27 && @input_bytes+line.bytesize<=27648
    @input_count+=1
    @input_bytes+=line.bytesize
    line
  end
  def require_eof
    @reader.require_eof
  end
  def write(fields)
    require_deadline(@deadline)
    frame=fields.join("|")+"\n"
    stop!("COMMAND_STOP","output frame bound") unless frame.bytesize<=1024 && ascii_frame?(frame)
    ceiling=@output_count<28 ? 28672 : 29696
    stop!("COMMAND_STOP","output aggregate bound") unless @output_count<29 && @output_bytes+frame.bytesize<=ceiling
    written=STDOUT.syswrite(frame)
    stop!("COMMAND_STOP","short output write") unless written==frame.bytesize
    @output_count+=1
    @output_bytes+=frame.bytesize
    frame
  end
end
def proof_rows(fields,leader_pid,leader_pgid)
  stop!("PROTOCOL_STOP","proof field count") unless fields.length==12
  raw=unb64(fields[3])
  stop!("PROTOCOL_STOP","proof byte length") unless canonical_decimal(fields[4]) && fields[4].to_i==raw.bytesize && raw.bytesize<=384
  stop!("PROTOCOL_STOP","proof hash") unless fields[5].match?(/\A[a-f0-9]{64}\z/) && sha256(raw)==fields[5]
  stop!("PROTOCOL_STOP","proof status") unless fields[6]=="0"
  stderr=unb64(fields[7])
  stop!("PROTOCOL_STOP","proof stderr") unless fields[8]=="0" && stderr.empty? && fields[9]==sha256(stderr)
  stop!("PROTOCOL_STOP","proof EOF") unless fields[10]=="1" && fields[11]=="1"
  stop!("PROTOCOL_STOP","proof framing") unless raw.bytesize>0 && raw.end_with?("\n") && ascii_frame?(raw)
  rows=raw.byteslice(0,raw.bytesize-1).split("\n").map do |line|
    match=/\A *([1-9][0-9]*) +([1-9][0-9]*) +([1-9][0-9]*) +0 +([0-9]+) +([IRSTUZ]s?) +((?:Sun|Mon|Tue|Wed|Thu|Fri|Sat) (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) (?: [1-9]|[12][0-9]|3[01]) (?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9] [0-9]{4}) +ruby {12}\z/.match(line)
    stop!("COMMAND_STOP","proof row grammar") if match.nil?
    {pid:match[1].to_i,ppid:match[2].to_i,pgid:match[3].to_i,uid:match[4].to_i,state:match[5],lstart:match[6]}
  end
  stop!("COMMAND_STOP","proof row count") unless rows.length>=1 && rows.length<=2
  stop!("COMMAND_STOP","proof row order") unless rows.map { |row| row[:pid] }==rows.map { |row| row[:pid] }.sort.uniq
  stop!("COMMAND_STOP","proof generation") unless rows.all? { |row| row[:pgid]==leader_pgid && row[:uid]==501 }
  rows
end
def profile_for(support,parent_path,child_path)
  lines=["(version 1)","(allow default)","(deny network*)"]
  if support
    paths=[parent_path,child_path].map { |path| "(literal \"#{path}\")" }.join(" ")
    ["network-bind","network-inbound","network-outbound"].each { |operation| lines << "(allow #{operation} #{paths})" }
  end
  lines.join("\n")+"\n"
end
def bounded_reap(generation,deadline)
  loop do
    result=Process.waitpid2(generation[:pid],Process::WNOHANG)
    unless result.nil?
      generation[:reaped]=true
      return result
    end
    require_deadline(deadline)
    IO.select(nil,nil,nil,0.001)
  end
end
def emergency_closeout(generation,first_fault,deadline)
  secondaries=[]
  return secondaries if generation.nil? || generation[:reaped]
  begin
    if generation[:state]==:forked_direct
      Process.kill("KILL",generation[:pid])
    else
      Process.kill("KILL",-generation[:pgid])
    end
  rescue Errno::ESRCH,Errno::EPERM => error
    secondaries << "SIGNAL_#{error.class.name.split("::").last}"
  rescue StandardError
    secondaries << "SIGNAL_OTHER"
  end
  begin
    bounded_reap(generation,deadline)
  rescue Errno::ECHILD
    secondaries << "REAP_ECHILD"
  rescue StandardError
    secondaries << "REAP_OTHER"
  end
  secondaries
end
def startup_details(report_reader,release_bytes,release_write)
  report=report_reader.nil? ? "".b : report_reader.retained
  report_eof=report_reader.nil? ? "0" : report_reader.eof ? "1" : "0"
  release=release_bytes || "".b
  release_eof=!release_write.nil? && release_write.closed? ? "1" : "0"
  [
    b64(report),report.bytesize.to_s,sha256(report),report_eof,
    b64(release),release.bytesize.to_s,sha256(release),release_eof
  ].join("|")
end
def launch_generation(root,index,token,leg,target_literal,deadline)
  support=leg!="denial"
  batch=File.join(root,"preflight","fifo","b#{(index+36).to_s.rjust(3,"0")}")
  receipt_path=File.join(batch,"receipt.fifo")
  stdout_path=File.join(batch,"stdout.fifo")
  stderr_path=File.join(batch,"stderr.fifo")
  ruby_root=File.join(root,"preflight","ruby")
  cwd=File.join(ruby_root,"cwd")
  home=File.join(ruby_root,"home")
  tmp=File.join(ruby_root,"tmp")
  parent_path=File.join(ruby_root,"parent","control.sock")
  child_path=File.join(ruby_root,"child","control.sock")
  [receipt_path,stdout_path,stderr_path,cwd,home,tmp].each { |path| stop!("COMMAND_STOP","missing launch path") unless File.exist?(path) }
  receipt=File.open(receipt_path,"w")
  stdout=File.open(stdout_path,"w")
  stderr=File.open(stderr_path,"w")
  report_read,report_write=IO.pipe
  release_read,release_write=IO.pipe
  report_reader=nil
  release_bytes="".b
  ordinary=leg=="support" || leg=="denial" ? ["GROUP_CONT"] : ["LEADER_KILL","SELECTED_SUFFIX_ALREADY_STOPPED","GROUP_CONT","GROUP_TERM"]
  generation={token:token,leg:leg,pid:nil,pgid:nil,state: :forked_direct,reaped:false,terminal_probes:0,observed_exit:false,expected_transitions:ordinary+["TERMINAL_KILL_PROBE_13","TERMINAL_KILL_PROBE_14"],transition_index:0}
  pid=fork do
    begin
      report_read.close
      release_write.close
      begin
        Process.setsid
      rescue SystemCallError
        begin
          report_write.syswrite("SETSID_STOP\n")
        rescue StandardError
        end
        exit! 78
      end
      report_write.sync=true
      report_write.syswrite("SETSID_OK\n")
      environment={
        "COMMAND_MODE"=>"unix2003",
        "HOME"=>home,
        "LANG"=>"C",
        "LC_ALL"=>"C",
        "TMPDIR"=>tmp,
        "TZ"=>"UTC",
        "__CF_USER_TEXT_ENCODING"=>"0x1F5:0x0:0x0"
      }
      profile=profile_for(support,parent_path,child_path)
      exec(
        environment,
        ["/usr/bin/sandbox-exec","/usr/bin/sandbox-exec"],
        "-p",profile,
        "/usr/bin/ruby",
        "--disable=gems,rubyopt,did_you_mean",
        "-I/System/Library/Frameworks/Ruby.framework/Versions/2.6/usr/lib/ruby/2.6.0/universal-darwin25",
        "-I/System/Library/Frameworks/Ruby.framework/Versions/2.6/usr/lib/ruby/2.6.0",
        "-rsocket",
        "-e",target_literal,
        "--",parent_path,child_path,leg,deadline.to_s,
        0=>"/dev/null",
        1=>stdout,
        2=>stderr,
        3=>receipt,
        4=>report_write,
        5=>release_read,
        chdir:cwd,
        close_others:true,
        unsetenv_others:true
      )
    rescue SystemCallError => error
      begin
        report_write.syswrite("EXEC_STOP|#{error.errno}\n")
      rescue StandardError
      end
      exit! 79
    rescue StandardError
      exit! 80
    end
  end
  generation[:pid]=pid
  receipt.close
  stdout.close
  stderr.close
  report_write.close
  release_read.close
  report_reader=BoundedReader.new(report_read,deadline,128,384)
  first=report_reader.read_record
  if first=="SETSID_STOP\n"
    report_reader.require_eof
    stop!("COMMAND_STOP","setsid failure",startup_details(report_reader,release_bytes,release_write))
  end
  stop!("COMMAND_STOP","setsid report",startup_details(report_reader,release_bytes,release_write)) unless first=="SETSID_OK\n"
  pgid=Process.getpgid(pid)
  sid=Process.getsid(pid)
  stop!("COMMAND_STOP","private group identity") unless pid==pgid && pid==sid
  generation[:pgid]=pgid
  generation[:state]=:group_verified
  second=report_reader.read_record
  unless second=="EXEC_OK\n"
    report_reader.require_eof if !second.nil? && second.start_with?("EXEC_STOP|")
    stop!("COMMAND_STOP","exec report",startup_details(report_reader,release_bytes,release_write))
  end
  generation[:state]=:exec_confirmed
  release_bytes="RELEASE_OK\n"
  stop!("COMMAND_STOP","release write") unless release_write.syswrite(release_bytes)==release_bytes.bytesize
  release_write.close
  third=report_reader.read_record
  stop!("COMMAND_STOP","released report") unless third=="RELEASED_OK\n"
  report_reader.require_eof
  report_read.close
  generation[:state]=:released
  report_bytes=first+second+third
  {generation:generation,report:report_bytes,release:release_bytes}
rescue StandardError => error
  secondaries=emergency_closeout(generation,error,deadline)
  details=startup_details(report_reader,release_bytes,release_write)
  details="#{details}|#{b64(secondaries.join(","))}"
  if error.is_a?(CustodyFault)
    raise CustodyFault.new(error.code,error.message,error.details.empty? ? details : error.details+"|"+b64(secondaries.join(",")))
  end
  raise CustodyFault.new("COMMAND_STOP","generation startup failed",details)
ensure
  [receipt,stdout,stderr,report_read,report_write,release_read,release_write].compact.each do |io|
    begin
      io.close unless io.closed?
    rescue StandardError
    end
  end
end
owner=nil
active=nil
begin
  stop!("BOOTSTRAP_STOP","argv") unless ARGV.length==4
  root,deadline_text,target_literal,target_hash=ARGV
  stop!("BOOTSTRAP_STOP","root") unless root.bytesize==41 && root.match?(/\A\/private\/tmp\/marrow-vsq-a-[a-f0-9]{8}\.[A-Za-z0-9]{6}\z/)
  stop!("BOOTSTRAP_STOP","deadline") unless canonical_decimal(deadline_text)
  deadline=deadline_text.to_i
  stop!("BOOTSTRAP_STOP","target literal") unless target_literal.ascii_only? && target_literal.bytesize<8192 && sha256(target_literal)==target_hash
  owner=FrameOwner.new(deadline)
  tokens=Array.new(4) { Random.urandom(16).unpack1("H*") }
  owner.write(["0","READY",*tokens])
  expected_input=0
  next_output=1
  next_leg=0
  sealed={}
  loop do
    raw=owner.read
    stop!(active.nil? ? "PROTOCOL_STOP" : "LOSS_STOP","command EOF") if raw.nil?
    fields=raw.byteslice(0,raw.bytesize-1).split("|",-1)
    stop!("PROTOCOL_STOP","input sequence") unless fields[0]==expected_input.to_s
    expected_input+=1
    kind=fields[1]
    if kind=="START_LEG"
      stop!("PROTOCOL_STOP","start shape") unless fields.length==4 && next_leg<4 && fields[2]==tokens[next_leg] && fields[3]==["support","denial","one-receipt","parent-loss"][next_leg] && active.nil? && !sealed.key?(fields[2])
      launched=launch_generation(root,next_leg,fields[2],fields[3],target_literal,deadline)
      active=launched[:generation]
      owner.write([next_output.to_s,"LEADER_OWNED",fields[2],b64(launched[:report]),b64(launched[:release]),"1","1"])
      next_output+=1
      next_leg+=1
      next
    end
    if kind=="FINAL_REAP"
      stop!("PROTOCOL_STOP","final reap shape") unless fields.length==3 && !active.nil? && fields[2]==active[:token] && active[:terminal_probes]==2 && active[:observed_exit] && active[:transition_index]==active[:expected_transitions].length
      waited_pid,status=Process.wait2(active[:pid])
      stop!("COMMAND_STOP","final reap identity") unless waited_pid==active[:pid]
      active[:reaped]=true
      active[:state]=:reaped
      status_kind=status.signaled? ? "SIGNALED" : status.exited? ? "EXITED" : "OTHER"
      status_code=status.signaled? ? status.termsig : status.exitstatus
      owner.write([next_output.to_s,"LEADER_REAPED",active[:token],status_kind,status_code.to_s])
      next_output+=1
      sealed[active[:token]]=true
      active=nil
      next
    end
    if kind=="CLOSE"
      stop!("PROTOCOL_STOP","close shape") unless fields.length==2 && active.nil? && next_leg==4 && sealed.length==4 && expected_input==27
      owner.require_eof
      owner.write([next_output.to_s,"CLOSEOUT"])
      next_output+=1
      stop!("COMMAND_STOP","clean frame counts") unless owner.input_count==27 && owner.output_count==28
      break
    end
    stop!("PROTOCOL_STOP","transition without generation") if active.nil? || fields[2]!=active[:token] || sealed.key?(fields[2])
    stop!("PROTOCOL_STOP","transition order") unless active[:expected_transitions][active[:transition_index]]==kind
    rows=proof_rows(fields,active[:pid],active[:pgid])
    if kind=="TERMINAL_KILL_PROBE_13" || kind=="TERMINAL_KILL_PROBE_14"
      stop!("PROTOCOL_STOP","terminal proof byte length") unless fields[4].to_i<=192
    end
    outcome=nil
    case kind
    when "LEADER_KILL"
      leader=rows.find { |row| row[:pid]==active[:pid] }
      stop!("COMMAND_STOP","leader not live") if leader.nil? || leader[:state].start_with?("Z")
      Process.kill("KILL",active[:pid])
      outcome="SIGNALED"
    when "SELECTED_SUFFIX_ALREADY_STOPPED"
      outcome="MARKED"
    when "GROUP_CONT"
      Process.kill("CONT",-active[:pgid])
      outcome="SIGNALED"
    when "GROUP_TERM"
      Process.kill("TERM",-active[:pgid])
      outcome="SIGNALED"
    when "TERMINAL_KILL_PROBE_13","TERMINAL_KILL_PROBE_14"
      stop!("COMMAND_STOP","terminal proof shape") unless rows.length==1 && rows[0][:pid]==active[:pid] && rows[0][:pgid]==active[:pgid] && rows[0][:state].start_with?("Z")
      active[:observed_exit]=true
      begin
        Process.kill("KILL",-active[:pgid])
        stop!("COMMAND_STOP","zombie-only group unexpectedly signalable")
      rescue Errno::EPERM
        outcome="NO_SIGNALABLE_GROUP_MEMBERS"
      rescue Errno::ESRCH
        stop!("COMMAND_STOP","pre-reap group ownership contradiction")
      rescue Errno::EINVAL
        stop!("COMMAND_STOP","invalid private group signal")
      end
      active[:terminal_probes]+=1
    else
      stop!("PROTOCOL_STOP","unknown transition")
    end
    active[:transition_index]+=1
    owner.write([next_output.to_s,"TRANSITION",active[:token],kind,outcome])
    next_output+=1
  end
rescue CustodyFault => error
  secondaries=emergency_closeout(active,error,defined?(deadline) ? deadline : monotonic_ns)
  begin
    kind=error.code=="BOOTSTRAP_STOP" ? "BOOTSTRAP_STOP" : error.code=="LOSS_STOP" ? "LOSS_STOP" : error.code=="DEADLINE_STOP" ? "DEADLINE_STOP" : error.code=="PROTOCOL_STOP" ? "PROTOCOL_STOP" : "COMMAND_STOP"
    if owner.nil?
      STDOUT.syswrite("0|BOOTSTRAP_STOP|#{error.code}|#{b64(error.details)}|#{b64(secondaries.join(","))}\n")
    else
      sequence=defined?(next_output) ? next_output : owner.output_count
      owner.write([sequence.to_s,kind,error.code,b64(error.details),b64(secondaries.join(","))])
    end
  rescue StandardError
  end
  exit 77
rescue StandardError => error
  secondaries=emergency_closeout(active,error,defined?(deadline) ? deadline : monotonic_ns)
  begin
    if owner.nil?
      STDOUT.syswrite("0|BOOTSTRAP_STOP|UNEXPECTED||#{b64(secondaries.join(","))}\n")
    else
      sequence=defined?(next_output) ? next_output : owner.output_count
      owner.write([sequence.to_s,"COMMAND_STOP","UNEXPECTED","",b64(secondaries.join(","))])
    end
  rescue StandardError
  end
  exit 78
end
`;

class HostAuthorityError extends Error {
  constructor(code, message, data = undefined) {
    super(message);
    this.name = "HostAuthorityError";
    this.code = code;
    this.data = boundedData(data);
  }
}

function fail(code, message, data = undefined) {
  throw new HostAuthorityError(code, message, data);
}

function requireCondition(condition, code, message, data = undefined) {
  if (!condition) fail(code, message, data);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function boundedData(value) {
  if (value === undefined) return undefined;
  try {
    const encoded = Buffer.from(canonicalJson(value));
    if (encoded.length <= 8_192) return value;
    return Object.freeze({
      serialization: "bounded",
      bytes: encoded.length,
      sha256: sha256(encoded),
    });
  } catch {
    return Object.freeze({ serialization: "failed" });
  }
}

function safeError(error) {
  const record = {
    code:
      typeof error?.code === "string" ? error.code : "host.internal",
    message: String(error?.message ?? error).slice(0, 1_024),
  };
  if (error?.data !== undefined) record.data = boundedData(error.data);
  if (error instanceof AggregateError) {
    record.errors = [...error.errors].slice(0, 16).map(safeError);
  }
  return Object.freeze(record);
}

function aggregate(primary, cleanupErrors) {
  const cleanup = cleanupErrors.filter(Boolean);
  if (primary === undefined && cleanup.length === 0) return undefined;
  if (primary !== undefined && cleanup.length === 0) return primary;
  return new AggregateError(
    primary === undefined ? cleanup : [primary, ...cleanup],
    "Phase A0 operation and cleanup did not both complete",
  );
}

function canonicalJson(value) {
  if (value === null) return "null";
  if (typeof value === "string" || typeof value === "boolean") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    requireCondition(
      Number.isFinite(value) && Number.isSafeInteger(value),
      "host.canonical_json",
      "canonical JSON accepts only safe integral numbers",
    );
    return JSON.stringify(value);
  }
  requireCondition(
    typeof value !== "bigint" && value !== undefined,
    "host.canonical_json",
    "BigInt and undefined require a typed string projection",
  );
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  requireCondition(
    typeof value === "object",
    "host.canonical_json",
    "unsupported canonical JSON value",
  );
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(",")}}`;
}

function requireExactKeys(value, expected, code, label) {
  requireCondition(
    value !== null &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      canonicalJson(Object.keys(value).sort()) ===
        canonicalJson([...expected].sort()),
    code,
    `${label} keys differ`,
    {
      actual:
        value !== null &&
        typeof value === "object" &&
        !Array.isArray(value)
          ? Object.keys(value).sort()
          : typeof value,
      expected: [...expected].sort(),
    },
  );
}

function randomToken() {
  return randomBytes(32).toString("hex");
}

function isCanonicalDecimal(value) {
  return typeof value === "string" && /^(0|[1-9][0-9]*)$/u.test(value);
}

function parseUnsignedBigInt(value, minimum = 1n, maximum = MAX_U64) {
  requireCondition(
    isCanonicalDecimal(value),
    "host.numeric_identity",
    "unsigned BigInt spelling is not canonical",
    { value: String(value).slice(0, 80) },
  );
  const parsed = BigInt(value);
  requireCondition(
    parsed >= minimum &&
      parsed <= maximum &&
      parsed.toString() === value,
    "host.numeric_identity",
    "unsigned BigInt is outside its admitted range",
    { value },
  );
  return parsed;
}

function parsePid(value) {
  requireCondition(
    typeof value === "string" && /^[1-9][0-9]*$/u.test(value),
    "host.pid",
    "PID spelling is not canonical",
  );
  const parsed = Number(value);
  requireCondition(
    Number.isSafeInteger(parsed) &&
      parsed >= PID_MIN &&
      parsed <= PID_MAX &&
      String(parsed) === value,
    "host.pid",
    "PID is outside the admitted range",
    { value },
  );
  return parsed;
}

function safeNumber(value, label) {
  requireCondition(
    typeof value === "bigint" &&
      value >= 0n &&
      value <= BigInt(Number.MAX_SAFE_INTEGER),
    "host.numeric_identity",
    `${label} is not a nonnegative safe integer`,
    { value: String(value) },
  );
  return Number(value);
}

function normalizeNativeDev(value) {
  requireCondition(
    typeof value === "bigint" && BigInt.asIntN(64, value) === value,
    "host.numeric_identity",
    "native device id is not signed-64",
    { value: String(value) },
  );
  return BigInt.asUintN(64, value);
}

function normalizeNativeIno(value) {
  requireCondition(
    typeof value === "bigint" && value >= 1n && value <= MAX_U64,
    "host.numeric_identity",
    "native inode id is outside unsigned-64",
    { value: String(value) },
  );
  return value;
}

function modeBits(stat) {
  return safeNumber(stat.mode & 0o7777n, "mode");
}

function typeName(stat) {
  if (stat.isFile()) return "file";
  if (stat.isDirectory()) return "directory";
  if (stat.isFIFO()) return "fifo";
  if (stat.isSocket()) return "socket";
  if (stat.isCharacterDevice()) return "character-device";
  if (stat.isSymbolicLink()) return "symlink";
  return "other";
}

function statFact(stat) {
  return Object.freeze({
    dev: normalizeNativeDev(stat.dev).toString(),
    ino: normalizeNativeIno(stat.ino).toString(),
    type: typeName(stat),
    mode: modeBits(stat),
    uid: safeNumber(stat.uid, "uid"),
    gid: safeNumber(stat.gid, "gid"),
    size: safeNumber(stat.size, "size"),
    nlink: safeNumber(stat.nlink, "nlink"),
  });
}

function sameFact(left, right) {
  return canonicalJson(left) === canonicalJson(right);
}

function stableObjectFact(fact) {
  return Object.freeze({
    dev: fact.dev,
    ino: fact.ino,
    type: fact.type,
    mode: fact.mode,
    uid: fact.uid,
    gid: fact.gid,
  });
}

function sameStableObject(left, right) {
  return sameFact(stableObjectFact(left), stableObjectFact(right));
}

function sameStableFile(left, right) {
  return (
    sameStableObject(left, right) &&
    left.nlink === right.nlink &&
    left.nlink === 1
  );
}

function lstatBig(path) {
  return lstatSync(path, { bigint: true });
}

function fstatBig(fd) {
  return fstatSync(fd, { bigint: true });
}

function fstatUnderDeadline(
  fd,
  deadline,
  label,
  readIdentity = fstatBig,
) {
  const raw = readIdentity(fd);
  deadline.check(
    "host.deadline",
    `${label} fstat returned after deadline`,
  );
  return statFact(raw);
}

function absentNoFollow(path) {
  try {
    lstatBig(path);
    return false;
  } catch (error) {
    if (error?.code === "ENOENT") return true;
    throw error;
  }
}

function checkedClose(fd, deadline = undefined) {
  closeSync(fd);
  deadline?.check("host.deadline", "close returned after deadline");
}

function writeAll(fd, bytes, deadline = undefined) {
  let offset = 0;
  while (offset < bytes.length) {
    const count = writeSync(fd, bytes, offset, bytes.length - offset);
    deadline?.check("host.deadline", "write returned after deadline");
    requireCondition(
      count > 0,
      "host.io",
      "write made no progress",
      { offset, bytes: bytes.length },
    );
    offset += count;
  }
}

class AbsoluteDeadline {
  constructor(label, endsNs) {
    this.label = label;
    this.endsNs = endsNs;
  }

  static fromNow(label, milliseconds) {
    return new AbsoluteDeadline(
      label,
      process.hrtime.bigint() + BigInt(milliseconds) * 1_000_000n,
    );
  }

  sub(label, milliseconds) {
    const localEnd =
      process.hrtime.bigint() + BigInt(milliseconds) * 1_000_000n;
    return new AbsoluteDeadline(
      `${this.label}/${label}`,
      localEnd < this.endsNs ? localEnd : this.endsNs,
    );
  }

  atMost(label, endsNs) {
    requireCondition(
      typeof endsNs === "bigint" && endsNs <= this.endsNs,
      "host.deadline",
      `${this.label}/${label} would extend its inherited deadline`,
      {
        inheritedDeadlineNs: this.endsNs.toString(),
        requestedDeadlineNs: String(endsNs),
      },
    );
    return new AbsoluteDeadline(`${this.label}/${label}`, endsNs);
  }

  remainingNs() {
    return this.endsNs - process.hrtime.bigint();
  }

  remainingMs() {
    const remaining = this.remainingNs();
    if (remaining <= 0n) return 0;
    const milliseconds = (remaining + 999_999n) / 1_000_000n;
    return Number(
      milliseconds > BigInt(2_147_483_647)
        ? 2_147_483_647n
        : milliseconds,
    );
  }

  check(code = "host.deadline", message = undefined) {
    requireCondition(
      process.hrtime.bigint() <= this.endsNs,
      code,
      message ?? `${this.label} exhausted its monotonic deadline`,
      { deadlineNs: this.endsNs.toString() },
    );
  }

  requireReserve(milliseconds, code = "host.deadline") {
    const reserve = BigInt(milliseconds) * 1_000_000n;
    const remaining = this.remainingNs();
    requireCondition(
      remaining >= reserve,
      code,
      `${this.label} lacks its required monotonic reserve`,
      {
        remainingNs: remaining.toString(),
        reserveNs: reserve.toString(),
      },
    );
  }
}

async function delay(deadline, milliseconds) {
  deadline.requireReserve(milliseconds);
  await new Promise((resolvePromise) => {
    setTimeout(resolvePromise, milliseconds);
  });
  deadline.check();
}

async function waitFor(deadline, promise, code, message) {
  deadline.check(code, message);
  let timer;
  try {
    const result = await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(
          () =>
            reject(
              new HostAuthorityError(code, message, {
                deadlineNs: deadline.endsNs.toString(),
              }),
            ),
          Math.max(1, deadline.remainingMs()),
        );
      }),
    ]);
    deadline.check(code, `${message} after deadline`);
    return result;
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

function nonthrowingOutcome(promise) {
  return promise.then(
    (value) => Object.freeze({ kind: "VALUE", value }),
    (error) => Object.freeze({ kind: "FAULT", error }),
  );
}

class S2ConcurrentSettlementLatch {
  constructor() {
    this.nextOrdinal = 0;
    this.roles = new Set();
  }

  capture(role, promise) {
    requireCondition(
      (role === "relay" || role === "worker") &&
        !this.roles.has(role) &&
        promise !== null &&
        typeof promise === "object" &&
        typeof promise.then === "function",
      "host.s2_relay",
      "S2 concurrent settlement authority differs",
      { role },
    );
    this.roles.add(role);
    return promise.then(
      (value) => this.settle(role, "VALUE", value),
      (error) => this.settle(role, "FAULT", error),
    );
  }

  settle(role, kind, payload) {
    const ordinal = this.nextOrdinal;
    this.nextOrdinal += 1;
    return kind === "VALUE"
      ? Object.freeze({ kind, ordinal, role, value: payload })
      : Object.freeze({ error: payload, kind, ordinal, role });
  }
}

class Capacity {
  constructor(label, maximum) {
    this.label = label;
    this.maximum = maximum;
    this.used = 0;
  }

  reserve(bytes) {
    requireCondition(
      Number.isSafeInteger(bytes) && bytes >= 0,
      "host.capacity",
      `${this.label} reservation is invalid`,
      { bytes },
    );
    requireCondition(
      this.used + bytes <= this.maximum,
      "host.capacity",
      `${this.label} capacity exceeded`,
      { used: this.used, requested: bytes, maximum: this.maximum },
    );
    this.used += bytes;
  }
}

const capacityReservationOwners = new WeakMap();
const s2RootInventoryOwners = new WeakMap();

function s2RootInventoryReceipt(receipt) {
  return Object.freeze({
    dev: receipt.dev,
    gid: receipt.gid,
    ino: receipt.ino,
    mode: receipt.mode,
    nlink: receipt.nlink,
    pathHash: receipt.pathHash,
    role: receipt.role,
    type: receipt.type,
    uid: receipt.uid,
  });
}

function beginS2RootInventory(capacity, initialReceipts) {
  requireCondition(
    capacity instanceof CapacityLedger &&
      !s2RootInventoryOwners.has(capacity) &&
      Array.isArray(initialReceipts) &&
      initialReceipts.length === 2,
    "host.root_inventory",
    "S2 root inventory owner or initial receipts differ",
  );
  const inventory = initialReceipts.map(s2RootInventoryReceipt);
  s2RootInventoryOwners.set(capacity, inventory);
  return inventory;
}

function snapshotS2RootInventory(capacity) {
  const inventory = s2RootInventoryOwners.get(capacity);
  requireCondition(
    Array.isArray(inventory) && inventory.length === 69,
    "host.root_inventory",
    "S2 root inventory cardinality differs",
    { roots: inventory?.length },
  );
  return Object.freeze(
    inventory.map((receipt) => Object.freeze({ ...receipt })),
  );
}

class CapacityLedger {
  constructor(initial = undefined) {
    this.maximum = CAPACITY_MAXIMA;
    this.reserved = Object.fromEntries(
      Object.keys(this.maximum).map((owner) => [owner, 0]),
    );
    this.completed = Object.fromEntries(
      Object.keys(this.maximum).map((owner) => [owner, 0]),
    );
    if (initial !== undefined) {
      requireCondition(
        Object.keys(initial).every((owner) =>
          Object.hasOwn(this.maximum, owner)
        ),
        "host.capacity",
        "initial capacity contains an unknown owner",
        { owners: Object.keys(initial).sort() },
      );
      for (const [owner, amount] of Object.entries(initial)) {
        requireCondition(
          Number.isSafeInteger(amount) &&
            amount >= 0 &&
            amount <= this.maximum[owner],
          "host.capacity",
          "initial capacity lies outside its closed maximum",
          { owner, amount, maximum: this.maximum[owner] },
        );
        this.reserved[owner] = amount;
        this.completed[owner] = amount;
      }
    }
  }

  reserve(owner, amount = 1) {
    requireCondition(
      Object.hasOwn(this.maximum, owner) &&
        Number.isSafeInteger(amount) &&
        amount >= 1,
      "host.capacity",
      "capacity reservation owner or amount differs",
      { owner, amount },
    );
    const next = this.reserved[owner] + amount;
    requireCondition(
      next <= this.maximum[owner],
      "host.capacity",
      "capacity reservation exceeds its closed maximum",
      {
        owner,
        reserved: this.reserved[owner],
        requested: amount,
        maximum: this.maximum[owner],
      },
    );
    this.reserved[owner] = next;
    const receipt = Object.freeze({
      kind: "CapacityReservation",
      owner,
      amount,
      ordinal: next,
    });
    capacityReservationOwners.set(receipt, this);
    return receipt;
  }

  reserveBundle(entries) {
    requireCondition(
      Array.isArray(entries) &&
        entries.length >= 1 &&
        new Set(entries.map(([owner]) => owner)).size ===
          entries.length &&
        entries.every(
          ([owner, amount]) =>
            Object.hasOwn(this.maximum, owner) &&
            Number.isSafeInteger(amount) &&
            amount >= 1,
        ),
      "host.capacity",
      "capacity bundle shape differs",
      { entries },
    );
    const next = entries.map(([owner, amount]) => [
      owner,
      amount,
      this.reserved[owner] + amount,
    ]);
    requireCondition(
      next.every(
        ([owner, _amount, value]) =>
          value <= this.maximum[owner],
      ),
      "host.capacity",
      "capacity bundle exceeds a closed maximum",
      {
        requested: Object.fromEntries(
          entries.map(([owner, amount]) => [
            owner,
            amount,
          ]),
        ),
        reserved: this.reserved,
        maximum: this.maximum,
      },
    );
    return Object.freeze(
      Object.fromEntries(
        next.map(([owner, amount, value]) => {
          this.reserved[owner] = value;
          const receipt = Object.freeze({
            kind: "CapacityReservation",
            owner,
            amount,
            ordinal: value,
          });
          capacityReservationOwners.set(receipt, this);
          return [owner, receipt];
        }),
      ),
    );
  }

  complete(receipt) {
    requireCondition(
      capacityReservationOwners.get(receipt) === this,
      "host.capacity",
      "capacity completion lacks its exact reservation owner",
    );
    capacityReservationOwners.delete(receipt);
    const next = this.completed[receipt.owner] + receipt.amount;
    requireCondition(
      next <= this.reserved[receipt.owner],
      "host.capacity",
      "capacity completion exceeds its reservation",
      { receipt, completed: this.completed[receipt.owner] },
    );
    this.completed[receipt.owner] = next;
    return next;
  }

  snapshot() {
    return Object.freeze({
      maximum: Object.freeze({ ...this.maximum }),
      reserved: Object.freeze({ ...this.reserved }),
      completed: Object.freeze({ ...this.completed }),
    });
  }
}

function reserveDirectory(capacity) {
  return capacity.reserve("directories");
}

function reserveRegularFile(capacity) {
  return capacity.reserve("regularFiles");
}

function reserveFifoBatch(capacity, inodeCount, pathBytes) {
  const bundle = capacity.reserveBundle([
    ["fifoBatches", 1],
    ["fifoInodes", inodeCount],
    ["fifoPathBytes", pathBytes],
  ]);
  return Object.freeze({
    batch: bundle.fifoBatches,
    inodes: bundle.fifoInodes,
    pathBytes: bundle.fifoPathBytes,
  });
}

function reserveNodeLeg(capacity, support) {
  const bundle = capacity.reserveBundle(
    support
      ? [
          ["nodeLegs", 1],
          ["sockets", 1],
        ]
      : [["nodeLegs", 1]],
  );
  return Object.freeze({
    leg: bundle.nodeLegs,
    socket: support ? bundle.sockets : null,
  });
}

function reserveRubyLeg(capacity, support) {
  const bundle = capacity.reserveBundle(
    support
      ? [
          ["rubyLegs", 1],
          ["sockets", 2],
        ]
      : [["rubyLegs", 1]],
  );
  return Object.freeze({
    leg: bundle.rubyLegs,
    sockets: support ? bundle.sockets : null,
  });
}

function reserveProof(capacity) {
  const bundle = capacity.reserveBundle([
    ["proofs", 1],
    ["psCaptures", 1],
  ]);
  return Object.freeze({
    proof: bundle.proofs,
    ps: bundle.psCaptures,
  });
}

function reserveS2CaptureAttempt(capacity) {
  const bundle = capacity.reserveBundle([
    ["captureAttempts", 1],
    ["psCaptures", 1],
  ]);
  return Object.freeze({
    attempt: bundle.captureAttempts,
    ps: bundle.psCaptures,
  });
}

function reserveS2Protocol(capacity) {
  const bundle = capacity.reserveBundle([
    ["rubyCustodySupervisors", 1],
    ["protocolSocketpairs", 3],
    ["protocolEndpoints", 6],
    ["startupPipes", 8],
    ["startupPipeEndpoints", 4],
    ["descriptorSlots", S2_DESCRIPTOR_CAPACITY],
  ]);
  return Object.freeze({
    supervisor: bundle.rubyCustodySupervisors,
    socketpairs: bundle.protocolSocketpairs,
    endpoints: bundle.protocolEndpoints,
    startupPipes: bundle.startupPipes,
    startupPipeEndpoints: bundle.startupPipeEndpoints,
    descriptorSlots: bundle.descriptorSlots,
  });
}

class EvidenceWriter {
  constructor(fd, identity, initial = undefined) {
    this.fd = fd;
    this.identity = identity;
    this.closed = false;
    this.sequence = initial?.sequence ?? 0;
    this.previousHash = initial?.previousHash ?? "0".repeat(64);
    this.total = new Capacity("evidence total", EVIDENCE_MAX_BYTES);
    this.partitions = new Map(
      Object.entries(PARTITION_CAPS).map(([name, cap]) => [
        name,
        new Capacity(`evidence partition ${name}`, cap),
      ]),
    );
    if (initial !== undefined) {
      this.total.used = initial.bytes;
      for (const [name, bytes] of Object.entries(initial.partitions)) {
        this.partitions.get(name).used = bytes;
      }
    }
  }

  add(partition, kind, facts, deadline = undefined) {
    requireCondition(
      !this.closed &&
        this.partitions.has(partition) &&
        /^[a-z0-9_.-]{1,80}$/u.test(kind),
      "host.evidence",
      "evidence record owner is invalid",
      { partition, kind },
    );
    const body = Object.freeze({
      schema: 1,
      sequence: this.sequence,
      partition,
      kind,
      previousHash: this.previousHash,
      facts,
    });
    const bodyBytes = Buffer.from(canonicalJson(body));
    const record = Object.freeze({
      ...body,
      hash: sha256(bodyBytes),
    });
    const encoded = Buffer.from(`${canonicalJson(record)}\n`);
    this.partitions.get(partition).reserve(encoded.length);
    this.total.reserve(encoded.length);
    writeAll(this.fd, encoded, deadline);
    this.sequence += 1;
    this.previousHash = record.hash;
    return record;
  }

  snapshot() {
    return Object.freeze({
      sequence: this.sequence,
      previousHash: this.previousHash,
      bytes: this.total.used,
      partitions: Object.freeze(
        Object.fromEntries(
          [...this.partitions.entries()].map(([name, capacity]) => [
            name,
            capacity.used,
          ]),
        ),
      ),
    });
  }

  finish(deadline = undefined) {
    requireCondition(!this.closed, "host.evidence", "evidence closed twice");
    fsyncSync(this.fd);
    deadline?.check("host.deadline", "evidence fsync returned after deadline");
    checkedClose(this.fd, deadline);
    this.closed = true;
    return this.snapshot();
  }
}

function openNoFollowRead(path) {
  return openSync(
    path,
    fsConstants.O_RDONLY |
      fsConstants.O_CLOEXEC |
      fsConstants.O_NOFOLLOW |
      fsConstants.O_NONBLOCK,
  );
}

function streamFd(fd, maximum, deadline, expectedBytes = undefined) {
  const buffer = Buffer.allocUnsafe(STREAM_BUFFER_BYTES);
  const hash = createHash("sha256");
  let bytes = 0;
  while (true) {
    const count = readSync(fd, buffer, 0, buffer.length, null);
    deadline.check("host.deadline", "stream read returned after deadline");
    if (count === 0) break;
    requireCondition(
      bytes + count <= maximum,
      "host.stream_bound",
      "stream exceeded its byte ceiling",
      { bytes: bytes + count, maximum },
    );
    bytes += count;
    hash.update(buffer.subarray(0, count));
  }
  if (expectedBytes !== undefined) {
    requireCondition(
      bytes === expectedBytes,
      "host.stream_bound",
      "stream byte count differs",
      { bytes, expectedBytes },
    );
  }
  return Object.freeze({ bytes, sha256: hash.digest("hex") });
}

function requireRegularFact(fact, expected, code) {
  requireCondition(
    fact.type === "file" &&
      fact.nlink === 1 &&
      fact.mode === expected.mode &&
      fact.uid === expected.uid &&
      fact.gid === expected.gid &&
      fact.size === expected.bytes,
    code,
    `regular identity drifted for ${expected.role}`,
    { fact, expected },
  );
}

function streamRegular(path, expected, deadline, baseline = undefined) {
  requireCondition(
    path === expected.path && realpathSync(path) === path,
    "host.static_identity",
    `canonical path drifted for ${expected.role}`,
    { path, realpath: realpathSync(path) },
  );
  deadline.check();
  const firstPath = statFact(lstatBig(path));
  requireRegularFact(firstPath, expected, "host.static_identity");
  const fd = openNoFollowRead(path);
  deadline.check("host.deadline", "open returned after deadline");
  let result;
  try {
    const before = fstatUnderDeadline(
      fd,
      deadline,
      `${expected.role} pre-stream`,
    );
    requireCondition(
      sameFact(firstPath, before),
      "host.static_identity",
      `path/fd identity differs for ${expected.role}`,
      { firstPath, before },
    );
    const streamed = streamFd(fd, expected.bytes, deadline, expected.bytes);
    const after = fstatUnderDeadline(
      fd,
      deadline,
      `${expected.role} post-stream`,
    );
    requireCondition(
      sameFact(before, after) && streamed.sha256 === expected.sha256,
      "host.static_identity",
      `streamed identity drifted for ${expected.role}`,
      { before, after, streamed, expectedSha256: expected.sha256 },
    );
    result = Object.freeze({
      role: expected.role,
      ...before,
      sha256: streamed.sha256,
    });
  } finally {
    checkedClose(fd, deadline);
  }
  const secondPath = statFact(lstatBig(path));
  deadline.check("host.deadline", "post-close lstat returned after deadline");
  requireCondition(
    realpathSync(path) === path &&
      sameFact(firstPath, secondPath) &&
      (baseline === undefined || sameFact(result, baseline)),
    "host.static_identity",
    `post-close identity drifted for ${expected.role}`,
    { result, baseline, secondPath },
  );
  return result;
}

function streamOwner(deadline, baseline = undefined) {
  requireCondition(
    realpathSync(OWNER_PATH) === OWNER_PATH,
    "host.owner_identity",
    "reviewed owner path is not canonical",
  );
  const firstPath = statFact(lstatBig(OWNER_PATH));
  requireCondition(
    firstPath.type === "file" &&
      firstPath.nlink === 1 &&
      firstPath.uid === HOST_UID &&
      firstPath.gid === HOST_GID &&
      firstPath.size <= OWNER_MAX_BYTES,
    "host.owner_identity",
    "reviewed owner metadata differs",
    { firstPath },
  );
  const fd = openNoFollowRead(OWNER_PATH);
  deadline.check("host.deadline", "owner open returned after deadline");
  let result;
  try {
    const before = fstatUnderDeadline(
      fd,
      deadline,
      "owner pre-stream",
    );
    requireCondition(
      sameFact(firstPath, before),
      "host.owner_identity",
      "owner path/fd identity differs",
    );
    const streamed = streamFd(fd, OWNER_MAX_BYTES, deadline, firstPath.size);
    const after = fstatUnderDeadline(
      fd,
      deadline,
      "owner post-stream",
    );
    requireCondition(
      sameFact(before, after),
      "host.owner_identity",
      "owner changed while streaming",
    );
    result = Object.freeze({
      role: "owner",
      ...before,
      sha256: streamed.sha256,
    });
  } finally {
    checkedClose(fd, deadline);
  }
  const secondPath = statFact(lstatBig(OWNER_PATH));
  requireCondition(
    sameFact(firstPath, secondPath) &&
      (baseline === undefined || sameFact(result, baseline)),
    "host.owner_identity",
    "owner changed after close",
    { result, baseline, secondPath },
  );
  return result;
}

function readCacheUuid(deadline) {
  const fd = openNoFollowRead(DYLD_CACHE);
  deadline.check();
  try {
    const header = Buffer.alloc(104);
    let offset = 0;
    while (offset < header.length) {
      const count = readSync(
        fd,
        header,
        offset,
        header.length - offset,
        offset,
      );
      deadline.check();
      requireCondition(
        count > 0,
        "host.cache_identity",
        "dyld cache header ended before UUID",
      );
      offset += count;
    }
    const hex = header.subarray(88, 104).toString("hex");
    return [
      hex.slice(0, 8),
      hex.slice(8, 12),
      hex.slice(12, 16),
      hex.slice(16, 20),
      hex.slice(20),
    ].join("-");
  } finally {
    checkedClose(fd, deadline);
  }
}

function staticPass(deadline, baseline = undefined) {
  requireCondition(
    process.execPath === NODE &&
      process.argv0 === NODE &&
      process.version === "v24.16.0" &&
      process.platform === "darwin" &&
      process.arch === "arm64",
    "host.node_identity",
    "bootstrap Node runtime identity drifted",
    {
      execPath: process.execPath,
      argv0: process.argv0,
      version: process.version,
      platform: process.platform,
      arch: process.arch,
    },
  );
  const files = FIXED_PINS.map((pin, index) =>
    streamRegular(
      pin.path,
      pin,
      deadline,
      baseline?.files?.[index],
    ),
  );
  const owner = streamOwner(deadline, baseline?.owner);
  const link = lstatBig(RUBY_DYLIB_LINK);
  requireCondition(
    link.isSymbolicLink() &&
      safeNumber(link.uid, "libruby uid") === 0 &&
      safeNumber(link.gid, "libruby gid") === 0 &&
      modeBits(link) === 0o755 &&
      readlinkSync(RUBY_DYLIB_LINK) === "../../Ruby",
    "host.cache_identity",
    "cache-backed libruby symlink sentinel drifted",
  );
  const cacheUuid = readCacheUuid(deadline);
  requireCondition(
    cacheUuid === "157e6d2e-2e5c-39b1-8f2a-8866ee228bed",
    "host.cache_identity",
    "dyld cache UUID drifted",
    { cacheUuid },
  );
  const fixedBytes = FIXED_PINS.reduce(
    (total, pin) => total + pin.bytes,
    0,
  );
  requireCondition(
    files.length === 16 && fixedBytes === 128_653_106,
    "host.static_identity",
    "fixed authority count/bytes drifted",
    { files: files.length, fixedBytes },
  );
  const result = Object.freeze({
    files: Object.freeze(files),
    owner,
    cacheUuid,
    librubyLink: "../../Ruby",
    digest: sha256(
      Buffer.from(
        canonicalJson({
          files,
          owner,
          cacheUuid,
          librubyLink: "../../Ruby",
        }),
      ),
    ),
  });
  if (baseline !== undefined) {
    requireCondition(
      result.digest === baseline.digest,
      "host.static_identity",
      "final static digest differs",
      { initial: baseline.digest, final: result.digest },
    );
  }
  return result;
}

function validateStaticSnapshot(snapshot) {
  requireExactKeys(
    snapshot,
    ["cacheUuid", "digest", "files", "librubyLink", "owner"],
    "host.s2_semantic",
    "static snapshot",
  );
  requireCondition(
    Array.isArray(snapshot.files) &&
      snapshot.files.length === FIXED_PINS.length &&
      snapshot.cacheUuid ===
        "157e6d2e-2e5c-39b1-8f2a-8866ee228bed" &&
      snapshot.librubyLink === "../../Ruby",
    "host.s2_semantic",
    "static snapshot authority differs",
  );
  snapshot.files.forEach((fact, index) => {
    const pin = FIXED_PINS[index];
    requireCondition(
      fact.role === pin.role &&
        fact.type === "file" &&
        fact.size === pin.bytes &&
        fact.uid === pin.uid &&
        fact.gid === pin.gid &&
        fact.mode === pin.mode &&
        fact.nlink === 1 &&
        fact.sha256 === pin.sha256 &&
        isCanonicalDecimal(fact.dev) &&
        isCanonicalDecimal(fact.ino),
      "host.s2_semantic",
      "static file projection differs",
      { index, role: pin.role, fact },
    );
  });
  requireCondition(
    snapshot.owner.role === "owner" &&
      snapshot.owner.type === "file" &&
      snapshot.owner.uid === HOST_UID &&
      snapshot.owner.gid === HOST_GID &&
      snapshot.owner.mode === 0o644 &&
      snapshot.owner.nlink === 1 &&
      snapshot.owner.size > 0 &&
      snapshot.owner.size <= OWNER_MAX_BYTES &&
      /^[a-f0-9]{64}$/u.test(snapshot.owner.sha256) &&
      isCanonicalDecimal(snapshot.owner.dev) &&
      isCanonicalDecimal(snapshot.owner.ino),
    "host.s2_semantic",
    "static owner projection differs",
  );
  const recomputed = sha256(
    Buffer.from(
      canonicalJson({
        files: snapshot.files,
        owner: snapshot.owner,
        cacheUuid: snapshot.cacheUuid,
        librubyLink: snapshot.librubyLink,
      }),
    ),
  );
  requireCondition(
    snapshot.digest === recomputed,
    "host.s2_semantic",
    "static snapshot digest is not derived",
  );
  return snapshot;
}

function readBoundedRegular(
  path,
  maximum,
  deadline,
  expectedIdentity = undefined,
) {
  const first = statFact(lstatBig(path));
  requireCondition(
    first.type === "file" && first.nlink === 1 && first.size <= maximum,
    "host.file_identity",
    "bounded regular file metadata differs",
    { first, maximum },
  );
  if (expectedIdentity !== undefined) {
    requireCondition(
      sameStableFile(first, expectedIdentity),
      "host.file_identity",
      "bounded regular file identity was replaced",
      { first, expectedIdentity },
    );
  }
  const fd = openNoFollowRead(path);
  deadline.check();
  const chunks = [];
  let bytes = 0;
  const hash = createHash("sha256");
  try {
    const before = fstatUnderDeadline(
      fd,
      deadline,
      "bounded regular pre-stream",
    );
    requireCondition(
      sameFact(first, before),
      "host.file_identity",
      "bounded file path/fd identity differs",
    );
    const buffer = Buffer.allocUnsafe(Math.min(STREAM_BUFFER_BYTES, maximum));
    while (true) {
      const count = readSync(fd, buffer, 0, buffer.length, null);
      deadline.check();
      if (count === 0) break;
      requireCondition(
        bytes + count <= maximum,
        "host.stream_bound",
        "bounded file exceeded its byte ceiling",
      );
      const chunk = Buffer.from(buffer.subarray(0, count));
      chunks.push(chunk);
      hash.update(chunk);
      bytes += count;
    }
    const after = fstatUnderDeadline(
      fd,
      deadline,
      "bounded regular post-stream",
    );
    requireCondition(
      sameFact(before, after) && bytes === before.size,
      "host.file_identity",
      "bounded file changed while reading",
      { before, after, bytes },
    );
  } finally {
    checkedClose(fd, deadline);
  }
  const second = statFact(lstatBig(path));
  requireCondition(
    sameFact(first, second),
    "host.file_identity",
    "bounded file changed after close",
  );
  return Object.freeze({
    body: Buffer.concat(chunks, bytes),
    bytes,
    sha256: hash.digest("hex"),
    identity: first,
  });
}

function parseEvidence(body) {
  requireCondition(
    body.length <= EVIDENCE_MAX_BYTES &&
      body.length > 0 &&
      body.at(-1) === 0x0a &&
      !body.includes(0x0d),
    "host.evidence",
    "evidence framing differs",
    { bytes: body.length },
  );
  const lines = body.toString("utf8").slice(0, -1).split("\n");
  const partitions = Object.fromEntries(
    Object.keys(PARTITION_CAPS).map((name) => [name, 0]),
  );
  let previousHash = "0".repeat(64);
  const records = [];
  let offset = 0;
  for (const [sequence, line] of lines.entries()) {
    const encoded = Buffer.from(`${line}\n`);
    offset += encoded.length;
    let record;
    try {
      record = JSON.parse(line);
    } catch (error) {
      fail("host.evidence", "evidence JSON is malformed", safeError(error));
    }
    requireCondition(
      canonicalJson(record) === line &&
        record.schema === 1 &&
        record.sequence === sequence &&
        typeof record.partition === "string" &&
        Object.hasOwn(PARTITION_CAPS, record.partition) &&
        typeof record.kind === "string" &&
        record.previousHash === previousHash &&
        typeof record.hash === "string" &&
        /^[a-f0-9]{64}$/u.test(record.hash),
      "host.evidence",
      "evidence record framing/sequence differs",
      { sequence },
    );
    const { hash, ...bodyRecord } = record;
    requireCondition(
      sha256(Buffer.from(canonicalJson(bodyRecord))) === hash,
      "host.evidence",
      "evidence record chain hash differs",
      { sequence },
    );
    partitions[record.partition] += encoded.length;
    requireCondition(
      partitions[record.partition] <=
        PARTITION_CAPS[record.partition],
      "host.evidence",
      "evidence partition exceeded",
      { partition: record.partition },
    );
    previousHash = hash;
    records.push(Object.freeze(record));
  }
  requireCondition(
    offset === body.length,
    "host.evidence",
    "evidence byte accounting differs",
  );
  return Object.freeze({
    records: Object.freeze(records),
    sequence: records.length,
    previousHash,
    bytes: body.length,
    partitions: Object.freeze(partitions),
    fileSha256: sha256(body),
  });
}

function openEvidenceAppend(path, identity, parsed, deadline) {
  const fd = openSync(
    path,
    fsConstants.O_WRONLY |
      fsConstants.O_APPEND |
      fsConstants.O_CLOEXEC |
      fsConstants.O_NOFOLLOW,
  );
  const fact = fstatUnderDeadline(
    fd,
    deadline,
    "evidence append",
  );
  requireCondition(
    sameStableFile(fact, identity) &&
      fact.size === parsed.bytes,
    "host.evidence",
    "evidence append identity differs",
    { fact, identity },
  );
  return new EvidenceWriter(fd, identity, parsed);
}

function rootFact(path, role) {
  const fact = statFact(lstatBig(path));
  requireCondition(
    fact.type === "directory" &&
      fact.mode === 0o700 &&
      fact.uid === HOST_UID &&
      fact.gid === HOST_GID &&
      fact.nlink >= 2 &&
      realpathSync(path) === path,
    "host.root_identity",
    `${role} directory identity differs`,
    { role, fact },
  );
  return Object.freeze({
    role,
    path,
    pathHash: sha256(Buffer.from(path)),
    ...fact,
  });
}

function sameRoot(path, receipt) {
  const current = rootFact(path, receipt.role);
  return (
    current.pathHash === receipt.pathHash &&
    sameStableObject(current, receipt)
  );
}

function assertAbsentTwice(path) {
  requireCondition(
    absentNoFollow(path) && absentNoFollow(path),
    "host.path_identity",
    "path was not absent in two nofollow observations",
    { pathHash: sha256(Buffer.from(path)) },
  );
}

function createDirectory(path, role, capacity) {
  const reservation = reserveDirectory(capacity);
  assertAbsentTwice(path);
  mkdirSync(path, { mode: 0o700 });
  chmodSync(path, 0o700);
  chownSync(path, HOST_UID, HOST_GID);
  const receipt = rootFact(path, role);
  const s2Inventory = s2RootInventoryOwners.get(capacity);
  if (s2Inventory !== undefined) {
    s2Inventory.push(s2RootInventoryReceipt(receipt));
  }
  capacity.complete(reservation);
  return receipt;
}

function removeDirectory(path, receipt) {
  requireCondition(
    sameRoot(path, receipt) &&
      readdirSync(path).length === 0 &&
      sameRoot(path, receipt) &&
      readdirSync(path).length === 0,
    "host.root_identity",
    "directory is changed or nonempty at retirement",
    { role: receipt.role },
  );
  rmdirSync(path);
  requireCondition(
    absentNoFollow(path),
    "host.teardown",
    "directory remained after retirement",
    { role: receipt.role },
  );
}

function createInvocation(runToken, deadline, capacity) {
  requireCondition(
    /^[a-f0-9]{64}$/u.test(runToken) &&
      capacity instanceof CapacityLedger,
    "host.run_token",
    "run token or invocation capacity owner differs",
  );
  const invocationReservation = reserveDirectory(capacity);
  const invocation = mkdtempSync(
    `/private/tmp/marrow-vsq-a-${runToken.slice(0, 8)}.`,
  );
  deadline.check();
  chmodSync(invocation, 0o700);
  deadline.check();
  chownSync(invocation, HOST_UID, HOST_GID);
  deadline.check();
  requireCondition(
    Buffer.byteLength(invocation) === 41 &&
      /^\/private\/tmp\/marrow-vsq-a-[a-f0-9]{8}\.[A-Za-z0-9]{6}$/u.test(
        invocation,
      ),
    "host.root_identity",
    "invocation root spelling differs",
    { bytes: Buffer.byteLength(invocation) },
  );
  const invocationReceipt = rootFact(invocation, "invocation");
  capacity.complete(invocationReservation);
  const evidenceRoot = join(invocation, "evidence");
  const evidenceRootReceipt = createDirectory(
    evidenceRoot,
    "evidence-root",
    capacity,
  );
  return Object.freeze({
    runToken,
    invocation,
    invocationReceipt,
    evidenceRoot,
    evidenceRootReceipt,
    capacity,
    counters: capacity.completed,
  });
}

function createEvidenceFile(state, deadline) {
  const path = join(state.evidenceRoot, "a0.jsonl");
  const reservation = reserveRegularFile(state.capacity);
  assertAbsentTwice(path);
  const fd = openSync(
    path,
    fsConstants.O_WRONLY |
      fsConstants.O_CREAT |
      fsConstants.O_EXCL |
      fsConstants.O_CLOEXEC |
      fsConstants.O_NOFOLLOW,
    0o600,
  );
  deadline.check();
  chmodSync(path, 0o600);
  chownSync(path, HOST_UID, HOST_GID);
  deadline.check();
  const identity = fstatUnderDeadline(
    fd,
    deadline,
    "new evidence file",
  );
  requireCondition(
    identity.type === "file" &&
      identity.mode === 0o600 &&
      identity.uid === HOST_UID &&
      identity.gid === HOST_GID &&
      identity.nlink === 1 &&
      identity.size === 0,
    "host.evidence",
    "new evidence identity differs",
    { identity },
  );
  const pathFact = statFact(lstatBig(path));
  requireCondition(
    sameFact(identity, pathFact),
    "host.evidence",
    "evidence path/fd identity differs",
  );
  state.capacity.complete(reservation);
  return Object.freeze({
    path,
    identity,
    writer: new EvidenceWriter(fd, identity),
  });
}

class Tombstones {
  constructor() {
    this.bytes = new Uint8Array(TOMBSTONE_BYTES);
    this.records = [];
  }

  has(pid) {
    requireCondition(
      Number.isInteger(pid) && pid >= PID_MIN && pid <= PID_MAX,
      "host.pid",
      "tombstone lookup PID differs",
      { pid },
    );
    const byte = Math.floor(pid / 8);
    const bit = pid % 8;
    return (this.bytes[byte] & 2 ** bit) !== 0;
  }

  add(pid, reason) {
    requireCondition(
      !this.has(pid) &&
        typeof reason === "string" &&
        /^[A-Z0-9_]{1,80}$/u.test(reason),
      "host.tombstone",
      "PID tombstone is duplicate or invalid",
      { pid, reason },
    );
    const byte = Math.floor(pid / 8);
    const bit = pid % 8;
    this.bytes[byte] |= 2 ** bit;
    this.records.push(Object.freeze({ pid, reason }));
  }

  digest() {
    const records = Object.freeze(
      this.records.map((record) =>
        Object.freeze({ pid: record.pid, reason: record.reason }),
      ),
    );
    return Object.freeze({
      bytes: this.bytes.length,
      count: records.length,
      sha256: sha256(this.bytes),
      bitsetBase64: Buffer.from(this.bytes).toString("base64"),
      records,
      recordsSha256: sha256(Buffer.from(canonicalJson(records))),
    });
  }
}

function requireDevNull(pathFact) {
  requireCondition(
    pathFact.type === "character-device" &&
      pathFact.mode === 0o666 &&
      pathFact.uid === 0 &&
      pathFact.gid === 0,
    "host.dev_null",
    "/dev/null identity differs",
    { pathFact },
  );
}

function openDevNull(flags, deadline) {
  const first = statFact(lstatBig(DEV_NULL));
  requireDevNull(first);
  const fd = openSync(
    DEV_NULL,
    flags | fsConstants.O_CLOEXEC | fsConstants.O_NOFOLLOW,
  );
  try {
    const fact = fstatUnderDeadline(fd, deadline, "/dev/null");
    const second = statFact(lstatBig(DEV_NULL));
    requireCondition(
      sameFact(first, fact) && sameFact(first, second),
      "host.dev_null",
      "/dev/null changed during handoff",
    );
    return Object.freeze({ fd, fact });
  } catch (error) {
    try {
      closeSync(fd);
    } catch (closeError) {
      throw aggregate(error, [closeError]);
    }
    throw error;
  }
}

function closedEnvironment(home, tmp) {
  return Object.freeze({
    COMMAND_MODE: "unix2003",
    HOME: home,
    LANG: "C",
    LC_ALL: "C",
    TMPDIR: tmp,
    TZ: "UTC",
    __CF_USER_TEXT_ENCODING: "0x1F5:0x0:0x0",
  });
}

function terminalLatch(child, label) {
  let resolveTerminal;
  const promise = new Promise((resolvePromise) => {
    resolveTerminal = resolvePromise;
  });
  const state = {
    error: null,
    terminal: null,
  };
  child.once("error", (error) => {
    state.error = safeError(error);
  });
  child.once("close", (code, signal) => {
    requireCondition(
      state.terminal === null,
      "host.process_terminal",
      `${label} emitted duplicate terminal state`,
    );
    state.terminal = Object.freeze({
      code,
      signal,
      error: state.error,
    });
    resolveTerminal(state.terminal);
  });
  return Object.freeze({
    promise,
    current() {
      return state.terminal;
    },
  });
}

function spawnExact({
  executable,
  args,
  cwd,
  env,
  stdio,
  detached = false,
  label,
  tombstones,
  onSpawn = undefined,
}) {
  requireCondition(
    onSpawn === undefined || typeof onSpawn === "function",
    "host.spawn",
    `${label} spawn adoption owner differs`,
  );
  const nonce = randomToken();
  let child;
  try {
    child = spawn(executable, args, {
      cwd,
      env,
      stdio,
      detached,
      uid: HOST_UID,
      gid: HOST_GID,
      shell: false,
      windowsHide: true,
    });
  } catch (error) {
    fail("host.spawn", `${label} spawn threw`, safeError(error));
  }
  const terminal = terminalLatch(child, label);
  const provisional = Object.freeze({
    child,
    pid: child.pid,
    label,
    nonce,
    terminal,
  });
  onSpawn?.(provisional);
  requireCondition(
    Number.isInteger(child.pid) &&
      child.pid >= PID_MIN &&
      child.pid <= PID_MAX &&
      !tombstones.has(child.pid),
    "host.spawn",
    `${label} returned an invalid or tombstoned PID`,
    { pid: child.pid },
  );
  return provisional;
}

function closeParentDescriptors(descriptors) {
  const errors = [];
  for (const descriptor of descriptors) {
    try {
      closeSync(descriptor);
    } catch (error) {
      errors.push(error);
    }
  }
  if (errors.length > 0) {
    throw new AggregateError(errors, "parent descriptor closeout failed");
  }
}

function requireExactTerminal(terminal, code, label, acceptedCode = 0) {
  requireCondition(
    terminal.error === null &&
      terminal.code === acceptedCode &&
      terminal.signal === null,
    code,
    `${label} terminal status differs`,
    terminal,
  );
}

async function settleDirectChild(
  launched,
  deadline,
  {
    normalMs,
    termMs,
    killMs,
    label = launched.label,
    allowSignal = false,
  },
) {
  const normal = deadline.sub(`${label}-normal`, normalMs);
  try {
    return Object.freeze({
      terminal: await waitFor(
        normal,
        launched.terminal.promise,
        "host.process_timeout",
        `${label} did not terminate in its normal window`,
      ),
      actions: Object.freeze([]),
    });
  } catch (error) {
    if (!(error instanceof HostAuthorityError) ||
        error.code !== "host.process_timeout") {
      throw error;
    }
  }
  const terminalAfterNormal = launched.terminal.current();
  if (terminalAfterNormal !== null) {
    return Object.freeze({
      terminal: terminalAfterNormal,
      actions: Object.freeze([]),
    });
  }
  const actions = [];
  const termWitness = Object.freeze({
    pid: launched.pid,
    nonce: launched.nonce,
    signal: "SIGTERM",
    monotonicNs: process.hrtime.bigint().toString(),
  });
  const termSent = launched.child.kill("SIGTERM");
  requireCondition(
    termSent,
    "host.signal_authority",
    `${label} direct TERM was refused`,
    termWitness,
  );
  actions.push(termWitness);
  try {
    const terminal = await waitFor(
      deadline.sub(`${label}-term`, termMs),
      launched.terminal.promise,
      "host.process_timeout",
      `${label} did not terminate after TERM`,
    );
    requireCondition(
      allowSignal || terminal.signal === "SIGTERM",
      "host.process_terminal",
      `${label} TERM terminal status differs`,
      terminal,
    );
    return Object.freeze({
      terminal,
      actions: Object.freeze(actions),
    });
  } catch (error) {
    if (!(error instanceof HostAuthorityError) ||
        error.code !== "host.process_timeout") {
      throw error;
    }
  }
  const terminalAfterTerm = launched.terminal.current();
  if (terminalAfterTerm !== null) {
    requireCondition(
      allowSignal || terminalAfterTerm.signal === "SIGTERM",
      "host.process_terminal",
      `${label} TERM terminal status differs`,
      terminalAfterTerm,
    );
    return Object.freeze({
      terminal: terminalAfterTerm,
      actions: Object.freeze(actions),
    });
  }
  const killWitness = Object.freeze({
    pid: launched.pid,
    nonce: launched.nonce,
    signal: "SIGKILL",
    monotonicNs: process.hrtime.bigint().toString(),
  });
  const killSent = launched.child.kill("SIGKILL");
  requireCondition(
    killSent,
    "host.signal_authority",
    `${label} direct KILL was refused`,
    killWitness,
  );
  actions.push(killWitness);
  const terminal = await waitFor(
    deadline.sub(`${label}-kill`, killMs),
    launched.terminal.promise,
    "host.process_timeout",
    `${label} did not reap after KILL`,
  );
  requireCondition(
    terminal.signal === "SIGKILL",
    "host.process_terminal",
    `${label} KILL terminal status differs`,
    terminal,
  );
  return Object.freeze({
    terminal,
    actions: Object.freeze(actions),
  });
}

function fifoReceipt(path, expectedParent, deadline) {
  requireCondition(
    resolve(path) === path &&
      dirname(path) === expectedParent.path &&
      /^[a-z]+\.fifo$/u.test(basename(path)),
    "host.fifo_identity",
    "FIFO pathname is outside its exact batch",
    { pathHash: sha256(Buffer.from(path)) },
  );
  const first = statFact(lstatBig(path));
  const fd = openSync(
    path,
    fsConstants.O_RDONLY |
      fsConstants.O_CLOEXEC |
      fsConstants.O_NOFOLLOW |
      fsConstants.O_NONBLOCK,
  );
  let descriptor;
  try {
    descriptor = fstatUnderDeadline(fd, deadline, "created FIFO");
  } finally {
    closeSync(fd);
  }
  const second = statFact(lstatBig(path));
  requireCondition(
    first.type === "fifo" &&
      first.mode === 0o600 &&
      first.uid === HOST_UID &&
      first.gid === HOST_GID &&
      first.nlink === 1 &&
      sameFact(first, descriptor) &&
      sameFact(first, second) &&
      sameRoot(expectedParent.path, expectedParent),
    "host.fifo_identity",
    "created FIFO identity differs",
    { first, descriptor, second },
  );
  return Object.freeze({
    path,
    pathHash: sha256(Buffer.from(path)),
    parent: expectedParent,
    ...first,
  });
}

function openFifoEndpoints(receipt, deadline) {
  let reader;
  let keeper;
  let writer;
  try {
    reader = openSync(
      receipt.path,
      fsConstants.O_RDONLY |
        fsConstants.O_CLOEXEC |
        fsConstants.O_NOFOLLOW |
        fsConstants.O_NONBLOCK,
    );
    keeper = openSync(
      receipt.path,
      fsConstants.O_RDWR |
        fsConstants.O_CLOEXEC |
        fsConstants.O_NOFOLLOW |
        fsConstants.O_NONBLOCK,
    );
    writer = openSync(
      receipt.path,
      fsConstants.O_WRONLY |
        fsConstants.O_CLOEXEC |
        fsConstants.O_NOFOLLOW |
        fsConstants.O_NONBLOCK,
    );
    for (const [role, fd] of [
      ["reader", reader],
      ["keeper", keeper],
      ["writer", writer],
    ]) {
      const fact = fstatUnderDeadline(
        fd,
        deadline,
        `FIFO ${role}`,
      );
      requireCondition(
        sameFact(fact, {
          dev: receipt.dev,
          ino: receipt.ino,
          type: receipt.type,
          mode: receipt.mode,
          uid: receipt.uid,
          gid: receipt.gid,
          size: receipt.size,
          nlink: receipt.nlink,
        }),
        "host.fifo_identity",
        `FIFO ${role} identity differs`,
        { fact, receipt },
      );
    }
    unlinkSync(receipt.path);
    requireCondition(
      absentNoFollow(receipt.path),
      "host.fifo_identity",
      "FIFO pathname remained after endpoint acquisition",
      { pathHash: receipt.pathHash },
    );
  } catch (error) {
    let closeError;
    try {
      closeParentDescriptors(
        [reader, keeper, writer].filter(
          (fd) => fd !== undefined,
        ),
      );
    } catch (caught) {
      closeError = caught;
    }
    throw aggregate(error, [closeError]);
  }
  return Object.freeze({ receipt, reader, keeper, writer });
}

function verifyFifoEofAndClose(endpoint, deadline) {
  let firstFault;
  try {
    const fact = fstatUnderDeadline(
      endpoint.reader,
      deadline,
      "FIFO closeout reader",
    );
    requireCondition(
      sameStableObject(fact, endpoint.receipt),
      "host.fifo_identity",
      "FIFO reader identity drifted at closeout",
      { fact, receipt: endpoint.receipt },
    );
    const probe = Buffer.alloc(1);
    let count;
    try {
      count = readSync(endpoint.reader, probe, 0, 1, null);
    } catch (error) {
      fail(
        "host.fifo_eof",
        "FIFO closeout did not observe exact EOF",
        safeError(error),
      );
    }
    deadline.check();
    requireCondition(
      count === 0,
      "host.fifo_eof",
      "FIFO closeout retained data or a writer",
      { count },
    );
  } catch (error) {
    firstFault = error;
  }
  let closeFault;
  try {
    checkedClose(endpoint.reader, deadline);
  } catch (error) {
    closeFault = error;
  }
  const combined = aggregate(firstFault, [closeFault]);
  if (combined !== undefined) throw combined;
}

function retirePartialFifoPath(path, batchReceipt) {
  if (absentNoFollow(path)) return;
  requireCondition(
    dirname(path) === batchReceipt.path &&
      sameRoot(batchReceipt.path, batchReceipt),
    "host.fifo_identity",
    "partial FIFO path escaped its receipted batch",
  );
  const first = statFact(lstatBig(path));
  const second = statFact(lstatBig(path));
  requireCondition(
    sameFact(first, second) &&
      first.type === "fifo" &&
      first.mode === 0o600 &&
      first.uid === HOST_UID &&
      first.gid === HOST_GID &&
      first.nlink === 1,
    "host.fifo_identity",
    "partial FIFO object is not safe to retire",
    { first, second },
  );
  unlinkSync(path);
  requireCondition(
    absentNoFollow(path),
    "host.fifo_identity",
    "partial FIFO pathname remained after retirement",
  );
}

class FifoManager {
  constructor(state, rootReceipt, tombstones, evidence) {
    this.state = state;
    this.rootReceipt = rootReceipt;
    this.tombstones = tombstones;
    this.evidence = evidence;
    this.used = new Set();
  }

  async create(index, names, deadline) {
    requireCondition(
      Number.isInteger(index) &&
        index >= 0 &&
        index < LIMITS.fifoBatches &&
        !this.used.has(index) &&
        Array.isArray(names) &&
        names.length >= 1 &&
        names.length <= 3 &&
        new Set(names).size === names.length &&
        names.every((name) =>
          ["receipt.fifo", "stdout.fifo", "stderr.fifo"].includes(name)
        ),
      "host.fifo_capacity",
      "FIFO batch shape or ordinal differs",
      { index, names },
    );
    const batchName = `b${String(index).padStart(3, "0")}`;
    const batchPath = join(this.rootReceipt.path, batchName);
    const paths = names.map((name) => join(batchPath, name));
    const argv = ["-m", "600", ...paths];
    const argvBytes = [MKFIFO, ...argv].reduce(
      (total, value) => total + Buffer.byteLength(value) + 1,
      0,
    );
    const pathBytes = paths.reduce(
      (total, value) => total + Buffer.byteLength(value),
      0,
    );
    requireCondition(
      paths.every(
        (path) => Buffer.byteLength(path) <= LIMITS.fifoLongestPath,
      ) &&
        argvBytes <= LIMITS.fifoArgvBytes,
      "host.fifo_capacity",
      "FIFO pathname/argv shape exceeded its local maximum",
      { argvBytes, pathBytes },
    );
    const reservations = reserveFifoBatch(
      this.state.capacity,
      paths.length,
      pathBytes,
    );
    this.used.add(index);
    let batchReceipt;
    let launched;
    let receipts = [];
    let endpoints = [];
    const cleanupErrors = [];
    try {
      batchReceipt = createDirectory(
        batchPath,
        `fifo-${batchName}`,
        this.state.capacity,
      );
      paths.forEach(assertAbsentTwice);
      const nulls = [];
      let nullOperationFault;
      try {
        for (const flag of [
          fsConstants.O_RDONLY,
          fsConstants.O_WRONLY,
          fsConstants.O_WRONLY,
        ]) {
          nulls.push(openDevNull(flag, deadline));
        }
        launched = spawnExact({
          executable: MKFIFO,
          args: argv,
          cwd: batchPath,
          env: closedEnvironment(
            this.state.invocation,
            this.state.invocation,
          ),
          stdio: nulls.map((entry) => entry.fd),
          label: `fifo-${batchName}`,
          tombstones: this.tombstones,
          onSpawn: (provisional) => {
            launched = provisional;
          },
        });
      } catch (error) {
        nullOperationFault = error;
        throw error;
      } finally {
        try {
          closeParentDescriptors(
            nulls.map((entry) => entry.fd),
          );
        } catch (closeError) {
          throw aggregate(nullOperationFault, [closeError]);
        }
      }
      const settled = await settleDirectChild(
        launched,
        deadline,
        {
          normalMs: 200,
          termMs: 100,
          killMs: 100,
          label: `fifo-${batchName}`,
        },
      );
      requireExactTerminal(
        settled.terminal,
        "host.fifo_create",
        `fifo-${batchName}`,
      );
      receipts = paths.map((path) =>
        fifoReceipt(path, batchReceipt, deadline)
      );
      for (const receipt of receipts) {
        endpoints.push(
          openFifoEndpoints(receipt, deadline),
        );
      }
      this.state.capacity.complete(reservations.batch);
      this.state.capacity.complete(reservations.inodes);
      this.state.capacity.complete(reservations.pathBytes);
      this.tombstones.add(launched.pid, "FIFO_DIRECT_REAP");
      this.evidence.add(
        "fifo_batches",
        "fifo.batch",
        {
          index,
          paths: receipts.map((receipt) => receipt.pathHash),
          pathBytes: paths.map((path) => Buffer.byteLength(path)),
          batchPathHash: batchReceipt.pathHash,
          argvBytes,
          pid: launched.pid,
          terminal: settled.terminal,
        },
        deadline,
      );
      for (const receipt of receipts) {
        this.state.fifoFacts.push(
          Object.freeze([
            index,
            receipt.pathHash,
            receipt.dev,
            receipt.ino,
          ]),
        );
      }
      return Object.freeze({
        index,
        batchPath,
        batchReceipt,
        endpoints: Object.freeze(endpoints),
      });
    } catch (primary) {
      for (const endpoint of endpoints) {
        for (const fd of [
          endpoint.reader,
          endpoint.keeper,
          endpoint.writer,
        ]) {
          try {
            closeSync(fd);
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
      }
      if (
        launched !== undefined &&
        launched.terminal.current() === null
      ) {
        try {
          await settleDirectChild(launched, deadline, {
            normalMs: 1,
            termMs: 100,
            killMs: 100,
            label: `fault-${batchName}`,
            allowSignal: true,
          });
        } catch (error) {
          cleanupErrors.push(error);
        }
      }
      if (
        launched !== undefined &&
        launched.terminal.current() !== null &&
        !this.tombstones.has(launched.pid)
      ) {
        try {
          this.tombstones.add(
            launched.pid,
            "FIFO_FAULT_DIRECT_REAP",
          );
        } catch (error) {
          cleanupErrors.push(error);
        }
      }
      if (batchReceipt !== undefined) {
        for (const path of paths) {
          try {
            retirePartialFifoPath(path, batchReceipt);
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
        try {
          removeDirectory(batchPath, batchReceipt);
        } catch (error) {
          cleanupErrors.push(error);
        }
      }
      throw aggregate(primary, cleanupErrors);
    }
  }

  async createExternal(index, names, deadline) {
    requireCondition(
      Number.isInteger(index) &&
        index >= 36 &&
        index <= 39 &&
        !this.used.has(index) &&
        canonicalJson(names) ===
          canonicalJson([
            "receipt.fifo",
            "stdout.fifo",
            "stderr.fifo",
          ]),
      "host.fifo_capacity",
      "external FIFO batch shape or ordinal differs",
      { index, names },
    );
    const batchName = `b${String(index).padStart(3, "0")}`;
    const batchPath = join(this.rootReceipt.path, batchName);
    const paths = names.map((name) => join(batchPath, name));
    const argv = ["-m", "600", ...paths];
    const argvBytes = [MKFIFO, ...argv].reduce(
      (total, value) => total + Buffer.byteLength(value) + 1,
      0,
    );
    const pathBytes = paths.reduce(
      (total, value) => total + Buffer.byteLength(value),
      0,
    );
    const reservations = reserveFifoBatch(
      this.state.capacity,
      paths.length,
      pathBytes,
    );
    this.used.add(index);
    let batchReceipt;
    let launched;
    let receipts = [];
    let endpoints = [];
    const cleanupErrors = [];
    try {
      batchReceipt = createDirectory(
        batchPath,
        `fifo-${batchName}`,
        this.state.capacity,
      );
      paths.forEach(assertAbsentTwice);
      const nulls = [];
      let nullOperationFault;
      try {
        for (const flag of [
          fsConstants.O_RDONLY,
          fsConstants.O_WRONLY,
          fsConstants.O_WRONLY,
        ]) {
          nulls.push(openDevNull(flag, deadline));
        }
        launched = spawnExact({
          executable: MKFIFO,
          args: argv,
          cwd: batchPath,
          env: closedEnvironment(
            this.state.invocation,
            this.state.invocation,
          ),
          stdio: nulls.map((entry) => entry.fd),
          label: `fifo-${batchName}`,
          tombstones: this.tombstones,
          onSpawn: (provisional) => {
            launched = provisional;
          },
        });
      } catch (error) {
        nullOperationFault = error;
        throw error;
      } finally {
        try {
          closeParentDescriptors(nulls.map((entry) => entry.fd));
        } catch (closeError) {
          throw aggregate(nullOperationFault, [closeError]);
        }
      }
      const settled = await settleDirectChild(
        launched,
        deadline,
        {
          normalMs: 200,
          termMs: 100,
          killMs: 100,
          label: `fifo-${batchName}`,
        },
      );
      requireExactTerminal(
        settled.terminal,
        "host.fifo_create",
        `fifo-${batchName}`,
      );
      receipts = paths.map((path) =>
        fifoReceipt(path, batchReceipt, deadline)
      );
      for (const receipt of receipts) {
        let reader;
        let keeper;
        try {
          reader = openSync(
            receipt.path,
            fsConstants.O_RDONLY |
              fsConstants.O_CLOEXEC |
              fsConstants.O_NOFOLLOW |
              fsConstants.O_NONBLOCK,
          );
          keeper = openSync(
            receipt.path,
            fsConstants.O_RDWR |
              fsConstants.O_CLOEXEC |
              fsConstants.O_NOFOLLOW |
              fsConstants.O_NONBLOCK,
          );
          for (const [role, fd] of [
            ["reader", reader],
            ["keeper", keeper],
          ]) {
            const fact = fstatUnderDeadline(
              fd,
              deadline,
              `external FIFO ${role}`,
            );
            requireCondition(
              sameStableObject(fact, receipt),
              "host.fifo_identity",
              `external FIFO ${role} identity differs`,
            );
          }
        } catch (error) {
          let closeFault;
          try {
            closeParentDescriptors(
              [reader, keeper].filter(
                (fd) => fd !== undefined,
              ),
            );
          } catch (caught) {
            closeFault = caught;
          }
          if (closeFault !== undefined) {
            throw new HostAuthorityError(
              typeof error?.code === "string"
                ? error.code
                : "host.fifo_identity",
              String(
                error?.message ??
                  "external FIFO endpoint acquisition failed",
              ),
              {
                firstFault: safeError(error),
                secondaryFaults: [safeError(closeFault)],
                retained: true,
              },
            );
          }
          throw error;
        }
        endpoints.push(Object.freeze({
          receipt,
          reader,
          keeper,
          writer: null,
        }));
      }
      this.state.capacity.complete(reservations.batch);
      this.state.capacity.complete(reservations.inodes);
      this.state.capacity.complete(reservations.pathBytes);
      this.tombstones.add(launched.pid, "FIFO_DIRECT_REAP");
      this.evidence.add(
        "fifo_batches",
        "fifo.batch",
        {
          index,
          externalWriter: true,
          paths: receipts.map((receipt) => receipt.pathHash),
          pathBytes: paths.map((path) => Buffer.byteLength(path)),
          batchPathHash: batchReceipt.pathHash,
          argvBytes,
          pid: launched.pid,
          terminal: settled.terminal,
        },
        deadline,
      );
      for (const receipt of receipts) {
        this.state.fifoFacts.push(
          Object.freeze([
            index,
            receipt.pathHash,
            receipt.dev,
            receipt.ino,
          ]),
        );
      }
      return Object.freeze({
        index,
        batchPath,
        batchReceipt,
        externalWriter: true,
        endpoints: Object.freeze(endpoints),
      });
    } catch (primary) {
      for (const endpoint of endpoints) {
        for (const fd of [endpoint.reader, endpoint.keeper]) {
          try {
            closeSync(fd);
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
      }
      if (
        launched !== undefined &&
        launched.terminal.current() === null
      ) {
        try {
          await settleDirectChild(launched, deadline, {
            normalMs: 1,
            termMs: 100,
            killMs: 100,
            label: `fault-${batchName}`,
            allowSignal: true,
          });
        } catch (error) {
          cleanupErrors.push(error);
        }
      }
      if (
        launched !== undefined &&
        launched.terminal.current() !== null &&
        !this.tombstones.has(launched.pid)
      ) {
        try {
          this.tombstones.add(
            launched.pid,
            "FIFO_FAULT_DIRECT_REAP",
          );
        } catch (error) {
          cleanupErrors.push(error);
        }
      }
      if (batchReceipt !== undefined) {
        for (const path of paths) {
          try {
            retirePartialFifoPath(path, batchReceipt);
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
        try {
          removeDirectory(batchPath, batchReceipt);
        } catch (error) {
          cleanupErrors.push(error);
        }
      }
      throw aggregate(primary, cleanupErrors);
    }
  }

  activateExternal(batch, deadline) {
    requireCondition(
      batch.externalWriter === true &&
        batch.endpoints.length === 3,
      "host.fifo_identity",
      "external FIFO activation shape differs",
    );
    const faults = [];
    for (const endpoint of batch.endpoints) {
      try {
        const pathFact = statFact(lstatBig(endpoint.receipt.path));
        const readerFact = fstatUnderDeadline(
          endpoint.reader,
          deadline,
          "external FIFO activation reader",
        );
        const keeperFact = fstatUnderDeadline(
          endpoint.keeper,
          deadline,
          "external FIFO activation keeper",
        );
        requireCondition(
          sameStableObject(pathFact, endpoint.receipt) &&
            sameStableObject(readerFact, endpoint.receipt) &&
            sameStableObject(keeperFact, endpoint.receipt),
          "host.fifo_identity",
          "external FIFO changed before activation",
        );
        unlinkSync(endpoint.receipt.path);
        requireCondition(
          absentNoFollow(endpoint.receipt.path),
          "host.fifo_identity",
          "external FIFO pathname remained after supervisor open",
        );
      } catch (error) {
        faults.push(error);
      }
      try {
        checkedClose(endpoint.keeper, deadline);
      } catch (error) {
        faults.push(error);
      }
    }
    if (faults.length > 0) {
      const [firstFault, ...secondaryFaults] = faults;
      throw new HostAuthorityError(
        typeof firstFault?.code === "string"
          ? firstFault.code
          : "host.fifo_identity",
        String(
          firstFault?.message ??
            "external FIFO activation failed",
        ),
        {
          firstFault: safeError(firstFault),
          secondaryFaults: secondaryFaults.map(safeError),
          retained: true,
        },
      );
    }
  }

  retireExternal(
    batch,
    deadline,
    intentionallyClosedReaders = new Set(),
  ) {
    requireCondition(
      batch.externalWriter === true,
      "host.fifo_identity",
      "non-external FIFO reached external retirement",
    );
    const errors = [];
    for (const [index, endpoint] of batch.endpoints.entries()) {
      if (!intentionallyClosedReaders.has(index)) {
        try {
          verifyFifoEofAndClose(endpoint, deadline);
        } catch (error) {
          errors.push(error);
        }
      }
    }
    try {
      removeDirectory(batch.batchPath, batch.batchReceipt);
    } catch (error) {
      errors.push(error);
    }
    if (errors.length > 0) {
      throw aggregate(errors[0], errors.slice(1));
    }
  }

  retire(batch, deadline, intentionallyClosedReaders = new Set()) {
    const errors = [];
    for (const [index, endpoint] of batch.endpoints.entries()) {
      if (!intentionallyClosedReaders.has(index)) {
        try {
          verifyFifoEofAndClose(endpoint, deadline);
        } catch (error) {
          errors.push(error);
        }
      }
    }
    try {
      removeDirectory(batch.batchPath, batch.batchReceipt);
    } catch (error) {
      errors.push(error);
    }
    if (errors.length > 0) {
      throw aggregate(errors[0], errors.slice(1));
    }
  }
}

function closeHandoff(batch) {
  closeParentDescriptors(
    batch.endpoints.flatMap((endpoint) => [
      endpoint.writer,
      endpoint.keeper,
    ]),
  );
}

function channelReader(
  fd,
  label,
  maximum,
  deadline,
) {
  let closed = false;
  const close = () => {
    if (!closed) {
      closeSync(fd);
      closed = true;
    }
  };
  const promise = (async () => {
    const chunks = [];
    let bytes = 0;
    const hash = createHash("sha256");
    while (true) {
      deadline.check("host.output_timeout");
      const remaining = maximum - bytes;
      const requested = Math.max(1, Math.min(4_096, remaining + 1));
      const buffer = Buffer.allocUnsafe(requested);
      let count;
      try {
        count = readSync(fd, buffer, 0, requested, null);
      } catch (error) {
        if (error?.code === "EAGAIN") {
          await delay(deadline, 1);
          continue;
        }
        throw error;
      }
      deadline.check("host.output_timeout");
      if (count === 0) {
        return Object.freeze({
          body: Buffer.concat(chunks, bytes),
          bytes,
          sha256: hash.digest("hex"),
          eof: true,
        });
      }
      requireCondition(
        bytes + count <= maximum,
        "host.output_overflow",
        `${label} exceeded its byte ceiling`,
        { bytes: bytes + count, maximum },
      );
      const retained = Buffer.from(buffer.subarray(0, count));
      chunks.push(retained);
      hash.update(retained);
      bytes += count;
    }
  })();
  return Object.freeze({
    promise,
    close,
    isClosed() {
      return closed;
    },
  });
}

class S2TransportBudget {
  constructor(
    label,
    frameMaximum,
    byteMaximum,
    frameByteMaximum = S2_PROTOCOL.frameBytes,
  ) {
    requireCondition(
      Number.isSafeInteger(frameByteMaximum) &&
        frameByteMaximum >= 1 &&
        frameByteMaximum <=
          S2_PROTOCOL.resultEnvelopeBytes,
      "host.s2_protocol",
      `${label} frame-byte maximum differs`,
    );
    this.label = label;
    this.frameByteMaximum = frameByteMaximum;
    this.frames = new Capacity(`${label} frames`, frameMaximum);
    this.bytes = new Capacity(`${label} bytes`, byteMaximum);
  }

  reserve(frame) {
    requireCondition(
      Buffer.isBuffer(frame) &&
        frame.length >= 1 &&
        frame.length <= this.frameByteMaximum &&
        frame.at(-1) === 0x0a &&
        frame.every(
          (byte) =>
            byte === 0x0a || (byte >= 0x20 && byte <= 0x7e),
        ),
      "host.s2_protocol",
      `${this.label} frame is outside its closed ASCII bound`,
      {
        bytes: Buffer.isBuffer(frame) ? frame.length : null,
      },
    );
    requireCondition(
      this.frames.used + 1 <= this.frames.maximum &&
        this.bytes.used + frame.length <=
          this.bytes.maximum,
      "host.s2_protocol",
      `${this.label} aggregate frame capacity exceeded`,
      {
        frames: this.frames.used,
        bytes: this.bytes.used,
        incoming: frame.length,
      },
    );
    this.reserveFrame(frame.length);
    this.bytes.reserve(frame.length);
  }

  reserveIncoming(chunk) {
    requireCondition(
      Buffer.isBuffer(chunk) &&
        chunk.length >= 1 &&
        this.bytes.used + chunk.length <=
          this.bytes.maximum,
      "host.s2_protocol",
      `${this.label} incoming byte capacity exceeded`,
      {
        bytes: Buffer.isBuffer(chunk) ? chunk.length : null,
        used: this.bytes.used,
        maximum: this.bytes.maximum,
      },
    );
    this.bytes.reserve(chunk.length);
  }

  reserveFrame(frameBytes) {
    requireCondition(
      Number.isSafeInteger(frameBytes) &&
        frameBytes >= 1 &&
        frameBytes <= this.frameByteMaximum &&
        this.frames.used + 1 <= this.frames.maximum,
      "host.s2_protocol",
      `${this.label} frame capacity exceeded`,
      {
        frameBytes,
        frames: this.frames.used,
        maximum: this.frames.maximum,
      },
    );
    this.frames.reserve(1);
  }

  snapshot() {
    return Object.freeze({
      frames: this.frames.used,
      bytes: this.bytes.used,
      frameMaximum: this.frames.maximum,
      frameByteMaximum: this.frameByteMaximum,
      byteMaximum: this.bytes.maximum,
    });
  }
}

class S2FrameReader {
  constructor(stream, label, budget, deadline) {
    requireCondition(
      stream !== null &&
        typeof stream === "object" &&
        typeof stream[Symbol.asyncIterator] === "function" &&
        budget instanceof S2TransportBudget &&
        deadline instanceof AbsoluteDeadline,
      "host.s2_protocol",
      "frame reader authority differs",
      { label },
    );
    this.stream = stream;
    this.label = label;
    this.budget = budget;
    this.deadline = deadline;
    this.iterator = stream[Symbol.asyncIterator]();
    this.pending = Buffer.alloc(0);
    this.ready = [];
    this.faultRaw = Buffer.alloc(0);
    this.ended = false;
    this.failed = false;
    this.firstFault = undefined;
  }

  async read() {
    if (this.failed) throw this.firstFault;
    requireCondition(
      !this.ended,
      "host.s2_protocol",
      `${this.label} read followed terminal EOF or fault`,
    );
    if (this.ready.length > 0) {
      return this.ready.shift();
    }
    let chargedFaultRaw;
    try {
      while (true) {
        requireCondition(
          this.pending.length < this.budget.frameByteMaximum,
          "host.s2_protocol",
          `${this.label} retained an overlong partial frame`,
          { bytes: this.pending.length },
        );
        const step = await waitFor(
          this.deadline,
          this.iterator.next(),
          "host.s2_protocol_timeout",
          `${this.label} read did not return`,
        );
        this.deadline.check(
          "host.s2_protocol_timeout",
          `${this.label} read returned late`,
        );
        if (step.done) {
          this.ended = true;
          requireCondition(
            this.pending.length === 0,
            "host.s2_protocol",
            `${this.label} ended with a partial frame`,
            {
              bytes: this.pending.length,
              sha256: sha256(this.pending),
            },
          );
          return null;
        }
        const chunk = step.value;
        this.budget.reserveIncoming(chunk);
        chargedFaultRaw = chunk;
        requireCondition(
          chunk.every(
            (byte) =>
              byte === 0x0a || (byte >= 0x20 && byte <= 0x7e),
          ),
          "host.s2_protocol",
          `${this.label} chunk contains a non-ASCII byte`,
          { chunk: chunk.length },
        );
        let offset = 0;
        while (offset < chunk.length) {
          const newline = chunk.indexOf(0x0a, offset);
          if (newline < 0) {
            const trailing = chunk.subarray(offset);
            requireCondition(
              this.pending.length + trailing.length <
                this.budget.frameByteMaximum,
              "host.s2_protocol",
              `${this.label} retained an overlong partial frame`,
              {
                pending: this.pending.length,
                trailing: trailing.length,
              },
            );
            this.pending = Buffer.concat(
              [this.pending, trailing],
              this.pending.length + trailing.length,
            );
            offset = chunk.length;
            continue;
          }
          const segment = chunk.subarray(offset, newline + 1);
          const frameBytes = this.pending.length + segment.length;
          this.budget.reserveFrame(frameBytes);
          const frame =
            this.pending.length === 0
              ? Buffer.from(segment)
              : Buffer.concat(
                  [this.pending, segment],
                  frameBytes,
                );
          this.ready.push(frame);
          this.pending = Buffer.alloc(0);
          offset = newline + 1;
        }
        if (this.ready.length > 0) {
          return this.ready.shift();
        }
      }
    } catch (error) {
      const raw =
        chargedFaultRaw === undefined
          ? this.pending
          : chargedFaultRaw;
      if (this.faultRaw.length === 0) {
        this.faultRaw = Buffer.from(
          raw.subarray(
            0,
            Math.min(
              raw.length,
              this.budget.frameByteMaximum,
            ),
          ),
        );
      }
      this.ready = [];
      this.pending = Buffer.alloc(0);
      this.failed = true;
      if (this.firstFault === undefined) {
        this.firstFault = error;
      }
      throw this.firstFault;
    }
  }

  faultSnapshot() {
    const raw =
      this.faultRaw.length > 0
        ? this.faultRaw
        : this.pending;
    return Object.freeze({
      raw: s2RawFact(raw),
      ended: this.ended,
      budget: this.budget.snapshot(),
    });
  }

  async requireEof() {
    if (this.failed) throw this.firstFault;
    if (this.ended) return;
    const frame = await this.read();
    try {
      requireCondition(
        frame === null,
        "host.s2_protocol",
        `${this.label} retained a trailing frame`,
        {
          bytes: frame?.length,
          sha256: frame === null ? undefined : sha256(frame),
        },
      );
    } catch (error) {
      if (this.faultRaw.length === 0 && frame !== null) {
        this.faultRaw = Buffer.from(frame);
      }
      this.ready = [];
      this.pending = Buffer.alloc(0);
      this.failed = true;
      if (this.firstFault === undefined) {
        this.firstFault = error;
      }
      throw this.firstFault;
    }
  }
}

async function writeS2Frame(
  stream,
  frame,
  label,
  budget,
  deadline,
) {
  budget.reserve(frame);
  const copied = Buffer.from(frame);
  const accepted = stream.write(copied);
  if (!accepted) {
    await waitFor(
      deadline,
      new Promise((resolvePromise, rejectPromise) => {
        const onDrain = () => {
          stream.off("error", onError);
          resolvePromise();
        };
        const onError = (error) => {
          stream.off("drain", onDrain);
          rejectPromise(error);
        };
        stream.once("drain", onDrain);
        stream.once("error", onError);
      }),
      "host.s2_protocol_timeout",
      `${label} write did not drain`,
    );
  }
  deadline.check(
    "host.s2_protocol_timeout",
    `${label} write returned late`,
  );
  return copied;
}

function encodeS2JsonFrame(value, maximum, label) {
  const encoded = Buffer.from(`${canonicalJson(value)}\n`, "ascii");
  requireCondition(
    encoded.length <= maximum &&
      encoded.every(
        (byte) =>
          byte === 0x0a || (byte >= 0x20 && byte <= 0x7e),
      ),
    "host.s2_protocol",
    `${label} JSON frame exceeds its closed ASCII bound`,
    { bytes: encoded.length, maximum },
  );
  return encoded;
}

function parseS2JsonFrame(frame, label) {
  requireCondition(
    Buffer.isBuffer(frame) &&
      frame.length >= 3 &&
      frame.at(-1) === 0x0a,
    "host.s2_protocol",
    `${label} JSON frame differs`,
  );
  const text = frame.toString("ascii").slice(0, -1);
  let value;
  try {
    value = JSON.parse(text);
  } catch (error) {
    fail(
      "host.s2_protocol",
      `${label} JSON is malformed`,
      safeError(error),
    );
  }
  requireCondition(
    canonicalJson(value) === text,
    "host.s2_protocol",
    `${label} JSON is noncanonical`,
  );
  return value;
}

function parseS2SupervisorFrame(frame) {
  requireCondition(
    Buffer.isBuffer(frame) &&
      frame.at(-1) === 0x0a &&
      frame.length <= S2_PROTOCOL.frameBytes,
    "host.s2_protocol",
    "supervisor frame shape differs",
  );
  const fields = frame.toString("ascii").slice(0, -1).split("|");
  requireCondition(
    fields.length >= 2 &&
      isCanonicalDecimal(fields[0]),
    "host.s2_protocol",
    "supervisor frame sequence differs",
    { fields: fields.length },
  );
  return Object.freeze({
    sequence: Number(fields[0]),
    kind: fields[1],
    fields: Object.freeze(fields),
    raw: Buffer.from(frame),
  });
}

const S2_SUPERVISOR_STOP_KINDS = Object.freeze([
  "BOOTSTRAP_STOP",
  "PROTOCOL_STOP",
  "COMMAND_STOP",
  "DEADLINE_STOP",
  "LOSS_STOP",
]);

function decodeCanonicalBase64(value, maximum, label) {
  requireCondition(
    typeof value === "string",
    "host.s2_protocol",
    `${label} base64 value differs`,
  );
  const decoded = Buffer.from(value, "base64");
  requireCondition(
    decoded.toString("base64") === value &&
      decoded.length <= maximum,
    "host.s2_protocol",
    `${label} base64 is noncanonical or over bound`,
    { bytes: decoded.length, maximum },
  );
  return decoded;
}

function parseS2StartupTranscript(details) {
  if (details.length === 0) return null;
  const fields = details.toString("ascii").split("|");
  requireCondition(
    fields.length === 9,
    "host.s2_protocol",
    "startup STOP transcript field count differs",
    { fields: fields.length },
  );
  const report = decodeCanonicalBase64(
    fields[0],
    S2_PROTOCOL.startupReportBytes,
    "startup report transcript",
  );
  const release = decodeCanonicalBase64(
    fields[4],
    S2_PROTOCOL.startupReleaseBytes,
    "startup release transcript",
  );
  const closeoutSecondaries = decodeCanonicalBase64(
    fields[8],
    128,
    "startup closeout secondaries",
  );
  requireCondition(
    isCanonicalDecimal(fields[1]) &&
      Number(fields[1]) === report.length &&
      /^[a-f0-9]{64}$/u.test(fields[2]) &&
      sha256(report) === fields[2] &&
      (fields[3] === "0" || fields[3] === "1") &&
      isCanonicalDecimal(fields[5]) &&
      Number(fields[5]) === release.length &&
      /^[a-f0-9]{64}$/u.test(fields[6]) &&
      sha256(release) === fields[6] &&
      (fields[7] === "0" || fields[7] === "1"),
    "host.s2_protocol",
    "startup STOP transcript relation differs",
  );
  return Object.freeze({
    report: s2RawFact(report),
    reportEof: fields[3] === "1",
    release: s2RawFact(release),
    releaseEof: fields[7] === "1",
    closeoutSecondaries: s2RawFact(closeoutSecondaries),
  });
}

function parseS2SupervisorStop(frame) {
  const parsed = parseS2SupervisorFrame(frame);
  if (!S2_SUPERVISOR_STOP_KINDS.includes(parsed.kind)) {
    return null;
  }
  requireCondition(
    Number.isSafeInteger(parsed.sequence) &&
      parsed.fields.length === 5 &&
      (parsed.fields[2] === parsed.kind ||
        (parsed.kind === "COMMAND_STOP" &&
          parsed.fields[2] === "UNEXPECTED")),
    "host.s2_protocol",
    "supervisor STOP frame schema differs",
    { fields: parsed.fields },
  );
  const details = decodeCanonicalBase64(
    parsed.fields[3],
    896,
    "supervisor STOP details",
  );
  const secondaries = decodeCanonicalBase64(
    parsed.fields[4],
    256,
    "supervisor STOP secondaries",
  );
  requireCondition(
    details.every(
      (byte) =>
        byte === 0x0a || (byte >= 0x20 && byte <= 0x7e),
    ) &&
      secondaries.every(
        (byte) =>
          byte === 0x0a || (byte >= 0x20 && byte <= 0x7e),
      ),
    "host.s2_protocol",
    "supervisor STOP payload is outside the ASCII domain",
  );
  const startup =
    parsed.kind === "COMMAND_STOP"
      ? parseS2StartupTranscript(details)
      : null;
  requireCondition(
    parsed.kind === "COMMAND_STOP" ||
      details.length === 0,
    "host.s2_protocol",
    "non-command STOP carried startup transcript bytes",
  );
  return Object.freeze({
    sequence: parsed.sequence,
    kind: parsed.kind,
    faultCode: parsed.fields[2],
    details: s2RawFact(details),
    secondaries: s2RawFact(secondaries),
    startup,
    raw: s2RawFact(frame),
  });
}

function s2RawFact(bytes) {
  return Object.freeze({
    base64: bytes.toString("base64"),
    bytes: bytes.length,
    sha256: sha256(bytes),
  });
}

function captureFacts(result) {
  return Object.freeze({
    bytes: result.bytes,
    sha256: result.sha256,
    eof: result.eof,
  });
}

function s2SecondaryFacts(errors) {
  requireCondition(
    Array.isArray(errors) && errors.length <= 8,
    "host.s2_protocol",
    "S2 secondary-fault count differs",
    { count: Array.isArray(errors) ? errors.length : null },
  );
  return Object.freeze(
    errors.map((error) => {
      const safe = safeError(error);
      const raw = Buffer.from(canonicalJson(safe));
      requireCondition(
        raw.length <= 16_384,
        "host.s2_protocol",
        "S2 secondary-fault projection exceeded its bound",
        { code: safe.code, bytes: raw.length },
      );
      return Object.freeze({
        bytes: raw.length,
        code: safe.code,
        sha256: sha256(raw),
      });
    }),
  );
}

function validateS2SecondaryFacts(facts, label) {
  requireCondition(
    Array.isArray(facts) &&
      facts.length <= 8 &&
      facts.every((fact) => {
        requireExactKeys(
          fact,
          ["bytes", "code", "sha256"],
          "host.s2_protocol",
          `${label} secondary fault`,
        );
        return (
          Number.isSafeInteger(fact.bytes) &&
          fact.bytes >= 1 &&
          fact.bytes <= 16_384 &&
          /^[A-Za-z0-9_.-]{1,80}$/u.test(fact.code) &&
          /^[a-f0-9]{64}$/u.test(fact.sha256)
        );
      }),
    "host.s2_protocol",
    `${label} secondary-fault facts differ`,
  );
}

const S2_DESCRIPTOR_PHASES = Object.freeze([
  Object.freeze({
    phase: "supervisor pre-spawn",
    node: 5,
    supervisor: 0,
    worker: 0,
    leader: 0,
    slots: 5,
  }),
  Object.freeze({
    phase: "supervisor spawned before parent close",
    node: 5,
    supervisor: 3,
    worker: 0,
    leader: 0,
    slots: 8,
  }),
  Object.freeze({
    phase: "supervisor steady",
    node: 2,
    supervisor: 3,
    worker: 0,
    leader: 0,
    slots: 5,
  }),
  Object.freeze({
    phase: "relay pre-worker-spawn",
    node: 4,
    supervisor: 3,
    worker: 0,
    leader: 0,
    slots: 7,
  }),
  Object.freeze({
    phase: "worker spawned before parent close",
    node: 4,
    supervisor: 3,
    worker: 1,
    leader: 0,
    slots: 8,
  }),
  Object.freeze({
    phase: "relay steady",
    node: 3,
    supervisor: 3,
    worker: 1,
    leader: 0,
    slots: 7,
  }),
  Object.freeze({
    phase: "leg handoff pre-fork",
    node: 3,
    supervisor: 11,
    worker: 1,
    leader: 0,
    slots: 15,
  }),
  Object.freeze({
    phase: "fork before inverse close",
    node: 3,
    supervisor: 11,
    worker: 1,
    leader: 11,
    slots: 26,
  }),
  Object.freeze({
    phase: "startup exchange",
    node: 3,
    supervisor: 5,
    worker: 1,
    leader: 6,
    slots: 15,
  }),
  Object.freeze({
    phase: "after release-write close",
    node: 3,
    supervisor: 4,
    worker: 1,
    leader: 6,
    slots: 14,
  }),
  Object.freeze({
    phase: "after target ACK/close",
    node: 3,
    supervisor: 4,
    worker: 1,
    leader: 4,
    slots: 12,
  }),
  Object.freeze({
    phase: "after report-read close",
    node: 3,
    supervisor: 3,
    worker: 1,
    leader: 4,
    slots: 11,
  }),
  Object.freeze({
    phase: "leader post-exec",
    node: 3,
    supervisor: 3,
    worker: 1,
    leader: 4,
    slots: 11,
  }),
]);

const S2_DESCRIPTOR_TRANSIENT_MAX = Math.max(
  ...S2_DESCRIPTOR_PHASES.map((entry) => entry.slots),
);
const S2_DESCRIPTOR_CAPACITY =
  33 + S2_DESCRIPTOR_TRANSIENT_MAX;

function s2DescriptorModel() {
  requireCondition(
    S2_DESCRIPTOR_PHASES.length === 13 &&
      S2_DESCRIPTOR_PHASES.every(
        (entry) =>
          Object.keys(entry).length === 6 &&
          entry.slots ===
            entry.node +
              entry.supervisor +
              entry.worker +
              entry.leader,
      ) &&
      S2_DESCRIPTOR_TRANSIENT_MAX === 26 &&
      S2_DESCRIPTOR_CAPACITY ===
        LIMITS.protocolDescriptors &&
      S2_DESCRIPTOR_CAPACITY === 59,
    "host.descriptor_capacity",
    "S2 descriptor phase model differs",
    {
      phases: S2_DESCRIPTOR_PHASES.length,
      transientMaximum: S2_DESCRIPTOR_TRANSIENT_MAX,
      capacity: S2_DESCRIPTOR_CAPACITY,
      configuredMaximum: LIMITS.protocolDescriptors,
    },
  );
  return Object.freeze({
    phases: S2_DESCRIPTOR_PHASES,
    formerS1: 33,
    transientMaximum: S2_DESCRIPTOR_TRANSIENT_MAX,
    capacity: S2_DESCRIPTOR_CAPACITY,
  });
}

function requireProtocolPeak(count, phase = undefined) {
  const model =
    phase === undefined ? undefined : s2DescriptorModel();
  const phaseEntry =
    phase === undefined
      ? undefined
      : model.phases.find((entry) => entry.phase === phase);
  requireCondition(
    Number.isInteger(count) &&
      count >= 0 &&
      count <= LIMITS.protocolDescriptors &&
      (phase === undefined || phaseEntry?.slots === count),
    "host.descriptor_capacity",
    "protocol descriptor ceiling exceeded",
    {
      count,
      phase,
      phaseSlots: phaseEntry?.slots,
      maximum: LIMITS.protocolDescriptors,
    },
  );
}

function sandboxLiteral(path) {
  requireCondition(
    typeof path === "string" &&
      /^\/[A-Za-z0-9._/-]{1,102}$/u.test(path) &&
      !path.includes("//") &&
      !path.includes("..") &&
      !path.includes('"') &&
      !path.includes("\\"),
    "host.profile",
    "sandbox literal path is outside the closed alphabet",
    { pathHash: sha256(Buffer.from(String(path))) },
  );
  return `(literal "${path}")`;
}

function sandboxProfile(kind, allowedPaths = []) {
  requireCondition(
    [
      "node-support",
      "node-denial",
      "ruby-support",
      "ruby-denial",
    ].includes(kind),
    "host.profile",
    "unknown sandbox profile class",
    { kind },
  );
  const paths = allowedPaths.map(sandboxLiteral);
  const lines = ["(version 1)", "(allow default)", "(deny network*)"];
  if (kind.startsWith("node-")) {
    lines.push("(deny process-fork)");
  }
  if (kind.endsWith("-support")) {
    requireCondition(
      (kind === "node-support" && paths.length === 1) ||
        (kind === "ruby-support" && paths.length === 2),
      "host.profile",
      "path-enabled profile received the wrong path count",
      { kind, paths: paths.length },
    );
    for (const operation of [
      "network-bind",
      "network-inbound",
      "network-outbound",
    ]) {
      lines.push(`(allow ${operation} ${paths.join(" ")})`);
    }
  } else {
    requireCondition(
      paths.length === 0,
      "host.profile",
      "deny profile received pathname authority",
      { kind, paths: paths.length },
    );
  }
  lines.push("");
  const profile = Buffer.from(lines.join("\n"), "utf8");
  requireCondition(
    profile.length <= 4_096 &&
      profile.at(-1) === 0x0a &&
      !profile.includes(0x0d),
    "host.profile",
    "inline sandbox profile framing/cap differs",
    { bytes: profile.length },
  );
  return Object.freeze({
    kind,
    bytes: profile,
    sha256: sha256(profile),
  });
}

function createCanary(state, preflightReceipt, deadline) {
  const path = join(preflightReceipt.path, "canary");
  const reservation = reserveRegularFile(state.capacity);
  assertAbsentTwice(path);
  const fd = openSync(
    path,
    fsConstants.O_WRONLY |
      fsConstants.O_CREAT |
      fsConstants.O_EXCL |
      fsConstants.O_CLOEXEC |
      fsConstants.O_NOFOLLOW,
    0o600,
  );
  try {
    writeAll(fd, CANARY_BYTES, deadline);
    fsyncSync(fd);
    deadline.check();
  } finally {
    checkedClose(fd, deadline);
  }
  chownSync(path, HOST_UID, HOST_GID);
  deadline.check();
  const identity = statFact(lstatBig(path));
  requireCondition(
    identity.type === "file" &&
      identity.mode === 0o600 &&
      identity.uid === HOST_UID &&
      identity.gid === HOST_GID &&
      identity.nlink === 1 &&
      identity.size === CANARY_BYTES.length &&
      sameRoot(preflightReceipt.path, preflightReceipt),
    "host.canary",
    "canary identity differs",
    { identity },
  );
  const reader = openNoFollowRead(path);
  const descriptor = fstatUnderDeadline(
    reader,
    deadline,
    "canary retained reader",
  );
  requireCondition(
    sameFact(identity, descriptor),
    "host.canary",
    "canary path/descriptor identity differs",
  );
  state.capacity.complete(reservation);
  return Object.freeze({
    path,
    pathHash: sha256(Buffer.from(path)),
    identity,
    reader,
  });
}

function checkCanary(canary, deadline) {
  const buffer = Buffer.alloc(CANARY_BYTES.length);
  let offset = 0;
  while (offset < buffer.length) {
    const count = readSync(
      canary.reader,
      buffer,
      offset,
      buffer.length - offset,
      offset,
    );
    deadline.check();
    requireCondition(
      count > 0,
      "host.canary",
      "canary ended before its fixed payload",
    );
    offset += count;
  }
  const fact = fstatUnderDeadline(
    canary.reader,
    deadline,
    "canary readback",
  );
  requireCondition(
    buffer.equals(CANARY_BYTES) &&
      sameFact(fact, canary.identity) &&
      sameFact(statFact(lstatBig(canary.path)), canary.identity),
    "host.canary",
    "canary bytes or identity drifted",
    {
      bytes: offset,
      sha256: sha256(buffer),
    },
  );
  return Object.freeze({
    pathHash: canary.pathHash,
    bytes: buffer.length,
    sha256: sha256(buffer),
  });
}

function retireCanary(canary, deadline) {
  checkCanary(canary, deadline);
  checkedClose(canary.reader, deadline);
  const first = statFact(lstatBig(canary.path));
  const fd = openNoFollowRead(canary.path);
  try {
    const descriptor = fstatUnderDeadline(
      fd,
      deadline,
      "canary closeout pre-stream",
    );
    const streamed = streamFd(
      fd,
      CANARY_BYTES.length,
      deadline,
      CANARY_BYTES.length,
    );
    const after = fstatUnderDeadline(
      fd,
      deadline,
      "canary closeout post-stream",
    );
    requireCondition(
      sameFact(first, canary.identity) &&
        sameFact(first, descriptor) &&
        sameFact(first, after) &&
        streamed.sha256 === sha256(CANARY_BYTES),
      "host.canary",
      "canary closeout identity differs",
    );
  } finally {
    checkedClose(fd, deadline);
  }
  requireCondition(
    sameFact(statFact(lstatBig(canary.path)), canary.identity),
    "host.canary",
    "canary path changed before retirement",
  );
  unlinkSync(canary.path);
  requireCondition(
    absentNoFollow(canary.path),
    "host.teardown",
    "canary remained after retirement",
  );
}

function nodeOutput(body, support) {
  requireCondition(
    Buffer.isBuffer(body) &&
      body.length > 0 &&
      body.at(-1) === 0x0a &&
      body.every(
        (byte) =>
          byte === 0x0a || (byte >= 0x20 && byte <= 0x7e),
      ),
    support ? "host.node_support" : "host.node_denial",
    "Node stream protocol byte domain differs",
    {
      bytes: Buffer.isBuffer(body) ? body.length : -1,
      sha256: Buffer.isBuffer(body) ? sha256(body) : "NONE",
    },
  );
  const text = body.toString("ascii");
  const match = support
    ? /^SUCCESS\|([1-9][0-9]*)\|([1-9][0-9]*)\|([1-9][0-9]*)\|([1-9][0-9]*)\n$/u.exec(
        text,
      )
    : /^DENIED\|([1-9][0-9]*)\|([1-9][0-9]*)\|EPERM\n$/u.exec(
        text,
      );
  requireCondition(
    match !== null,
    support ? "host.node_support" : "host.node_denial",
    "Node stream protocol output differs",
    { bytes: body.length, sha256: sha256(body) },
  );
  const result = {
    result: support ? "SUCCESS" : "DENIED",
    pid: parsePid(match[1]),
    ppid: parsePid(match[2]),
  };
  if (support) {
    result.dev = parseUnsignedBigInt(match[3]).toString();
    result.ino = parseUnsignedBigInt(match[4]).toString();
  }
  return Object.freeze(result);
}

async function runNodeLeg(
  worker,
  fifo,
  evidence,
  tombstones,
  support,
  previousRoot,
) {
  const deadline = worker.deadline.sub(
    support ? "node-support" : "node-denial",
    DEADLINE_MS.node,
  );
  const reservations = reserveNodeLeg(worker.capacity, support);
  const rootPath = join(worker.preflight, "node-stream");
  const root = createDirectory(
    rootPath,
    support ? "node-support" : "node-denial",
    worker.capacity,
  );
  if (previousRoot !== undefined) {
    requireCondition(
      root.dev !== previousRoot.dev || root.ino !== previousRoot.ino,
      "host.root_identity",
      "Node leg root inode was reused",
    );
  }
  const socketPath = join(rootPath, "control.sock");
  requireCondition(
    Buffer.byteLength(socketPath) === 76 &&
      /^\/private\/tmp\/marrow-vsq-a-[a-f0-9]{8}\.[A-Za-z0-9]{6}\/preflight\/node-stream\/control\.sock$/u.test(
        socketPath,
      ),
    "host.node_path",
    "Node socket spelling differs",
    { bytes: Buffer.byteLength(socketPath) },
  );
  assertAbsentTwice(socketPath);
  const batch = await fifo.create(
    support ? 40 : 41,
    ["stdout.fifo", "stderr.fifo"],
    deadline.sub("fifo", DEADLINE_MS.fifoBatch),
  );
  const stdout = channelReader(
    batch.endpoints[0].reader,
    "Node stdout",
    256,
    deadline,
  );
  const stderr = channelReader(
    batch.endpoints[1].reader,
    "Node stderr",
    256,
    deadline,
  );
  const profile = sandboxProfile(
    support ? "node-support" : "node-denial",
    support ? [socketPath] : [],
  );
  const stdin = openDevNull(fsConstants.O_RDONLY, deadline);
  let launched;
  let launchFault;
  try {
    requireProtocolPeak(14);
    launched = spawnExact({
      executable: SANDBOX_EXEC,
      args: [
        "-p",
        profile.bytes.toString("utf8"),
        NODE,
        "-e",
        NODE_LITERAL,
        "--",
        socketPath,
      ],
      cwd: rootPath,
      env: closedEnvironment(worker.invocation, worker.invocation),
      stdio: [
        stdin.fd,
        batch.endpoints[0].writer,
        batch.endpoints[1].writer,
      ],
      label: support ? "node-support" : "node-denial",
      tombstones,
      onSpawn: (provisional) => {
        launched = provisional;
      },
    });
  } catch (error) {
    launchFault = error;
  } finally {
    checkedClose(stdin.fd, deadline);
  }
  closeHandoff(batch);
  const [settledOutcome, stdoutOutcome, stderrOutcome] =
    await Promise.all([
      launched === undefined
        ? Object.freeze({
            kind: "FAULT",
            error:
              launchFault ??
              new HostAuthorityError(
                "host.spawn",
                "Node leg spawn failed before direct-child adoption",
              ),
          })
        : nonthrowingOutcome(
            settleDirectChild(launched, deadline, {
              normalMs: 8_000,
              termMs: 500,
              killMs: 500,
              label: launched.label,
            }),
          ),
      nonthrowingOutcome(stdout.promise),
      nonthrowingOutcome(stderr.promise),
    ]);
  let settled;
  let stdoutResult;
  let stderrResult;
  let output;
  let firstFault;
  try {
    if (launchFault !== undefined) throw launchFault;
    requireCondition(
      settledOutcome.kind === "VALUE" &&
        stdoutOutcome.kind === "VALUE" &&
        stderrOutcome.kind === "VALUE",
      "host.node_output",
      "Node child/output outcome faulted",
      {
        terminal:
          settledOutcome.kind === "FAULT"
            ? safeError(settledOutcome.error)
            : "VALUE",
        stdout:
          stdoutOutcome.kind === "FAULT"
            ? safeError(stdoutOutcome.error)
            : "VALUE",
        stderr:
          stderrOutcome.kind === "FAULT"
            ? safeError(stderrOutcome.error)
            : "VALUE",
      },
    );
    settled = settledOutcome.value;
    stdoutResult = stdoutOutcome.value;
    stderrResult = stderrOutcome.value;
    requireCondition(
      settled.terminal.error === null &&
        settled.terminal.code === 0 &&
        settled.terminal.signal === null,
      support ? "host.node_support" : "host.node_denial",
      `${launched.label} terminal status differs`,
      {
        terminal: settled.terminal,
        stdout: captureFacts(stdoutResult),
        stderr: captureFacts(stderrResult),
        stdoutBase64: stdoutResult.body.toString("base64"),
        stderrBase64: stderrResult.body.toString("base64"),
      },
    );
    requireCondition(
      stderrResult.eof &&
        stderrResult.bytes === 0 &&
        stdoutResult.eof,
      "host.node_output",
      "Node output EOF/emptiness differs",
      {
        stdout: captureFacts(stdoutResult),
        stderr: captureFacts(stderrResult),
      },
    );
    output = nodeOutput(stdoutResult.body, support);
    requireCondition(
      output.pid === launched.pid &&
        output.ppid === process.pid &&
        absentNoFollow(socketPath) &&
        absentNoFollow(socketPath),
      support ? "host.node_support" : "host.node_denial",
      "Node identity, parent, or pathname retirement differs",
      {
        output,
        launchedPid: launched.pid,
        workerPid: process.pid,
      },
    );
  } catch (error) {
    firstFault = error;
  }
  const cleanupFaults = [];
  if (
    launched !== undefined &&
    launched.terminal.current() === null
  ) {
    try {
      settled = await settleDirectChild(launched, deadline, {
        normalMs: 1,
        termMs: 500,
        killMs: 500,
        label: `${launched.label}-fault-closeout`,
        allowSignal: true,
      });
    } catch (error) {
      cleanupFaults.push(error);
    }
  }
  if (
    launched !== undefined &&
    launched.terminal.current() !== null &&
    !tombstones.has(launched.pid)
  ) {
    try {
      tombstones.add(
        launched.pid,
        support ? "NODE_SUPPORT_REAP" : "NODE_DENIAL_REAP",
      );
    } catch (error) {
      cleanupFaults.push(error);
    }
  }
  try {
    fifo.retire(batch, deadline);
  } catch (error) {
    cleanupFaults.push(error);
  }
  if (absentNoFollow(socketPath)) {
    try {
      removeDirectory(rootPath, root);
    } catch (error) {
      cleanupFaults.push(error);
    }
  } else {
    cleanupFaults.push(
      new HostAuthorityError(
        "host.node_cleanup",
        "Node socket pathname survived direct-child closeout",
        { socketPathHash: sha256(Buffer.from(socketPath)) },
      ),
    );
  }
  if (firstFault !== undefined || cleanupFaults.length > 0) {
    throw aggregate(
      firstFault ??
        new HostAuthorityError(
          "host.node_cleanup",
          "Node leg cleanup failed after a valid operation",
        ),
      cleanupFaults,
    );
  }
  worker.capacity.complete(reservations.leg);
  if (reservations.socket !== null) {
    worker.capacity.complete(reservations.socket);
  }
  evidence.add(
    "node",
    support ? "node.support" : "node.denial",
    {
      literalSha256: sha256(Buffer.from(NODE_LITERAL)),
      profileSha256: profile.sha256,
      profileRaw: s2RawFact(profile.bytes),
      socketPathHash: sha256(Buffer.from(socketPath)),
      rootPathHash: sha256(Buffer.from(rootPath)),
      argvProjection: Object.freeze([
        "-p",
        profile.sha256,
        NODE,
        "-e",
        sha256(Buffer.from(NODE_LITERAL)),
        "--",
        sha256(Buffer.from(socketPath)),
      ]),
      argvSha256: sha256(
        Buffer.from(
          canonicalJson([
            "-p",
            profile.sha256,
            NODE,
            "-e",
            sha256(Buffer.from(NODE_LITERAL)),
            "--",
            sha256(Buffer.from(socketPath)),
          ]),
        ),
      ),
      terminal: settled.terminal,
      stdout: captureFacts(stdoutResult),
      stderr: captureFacts(stderrResult),
      stdoutRaw: s2RawFact(stdoutResult.body),
      stderrRaw: s2RawFact(stderrResult.body),
      output,
      root: {
        dev: root.dev,
        ino: root.ino,
        pathHash: root.pathHash,
      },
    },
    deadline,
  );
  return root;
}

function parseCanonicalSafeInteger(value, minimum, maximum, label) {
  requireCondition(
    typeof value === "string" && /^(0|[1-9][0-9]*)$/u.test(value),
    "host.numeric_identity",
    `${label} spelling is not canonical`,
    { value: String(value).slice(0, 80) },
  );
  const parsed = Number(value);
  requireCondition(
    Number.isSafeInteger(parsed) &&
      parsed >= minimum &&
      parsed <= maximum &&
      String(parsed) === value,
    "host.numeric_identity",
    `${label} is outside its admitted range`,
    { value },
  );
  return parsed;
}

function parseRubyReceiptLine(line) {
  requireCondition(
    Buffer.byteLength(line) <= 70 &&
      line.endsWith("\n") &&
      !line.slice(0, -1).includes("\n") &&
      !line.includes("\r"),
    "host.ruby_receipt",
    "Ruby receipt record framing differs",
    { bytes: Buffer.byteLength(line) },
  );
  const match =
    /^([PC])\|([SD])\|([1-9][0-9]*)\|([1-9][0-9]*)\|([1-9][0-9]*)\|([1-9][0-9]*)\|([^|\n]+)\|([^|\n]+)\n$/u.exec(
      line,
    );
  requireCondition(
    match !== null,
    "host.ruby_receipt",
    "Ruby receipt grammar differs",
    { bytes: Buffer.byteLength(line), sha256: sha256(Buffer.from(line)) },
  );
  const record = {
    role: match[1],
    branch: match[2],
    pid: parsePid(match[3]),
    ppid: parsePid(match[4]),
    pgid: parsePid(match[5]),
    sid: parsePid(match[6]),
  };
  if (record.branch === "S") {
    record.dev = parseUnsignedBigInt(match[7]).toString();
    record.ino = parseUnsignedBigInt(match[8]).toString();
  } else {
    requireCondition(
      match[7] === "bind" && match[8] === "1",
      "host.ruby_receipt",
      "Ruby denial receipt is not exact EPERM bind",
      { detailA: match[7], detailB: match[8] },
    );
    record.denial = "EPERM";
  }
  return Object.freeze(record);
}

const RECEIPT_SOURCE_FIELDS = Object.freeze([
  "roles",
  "trailingLength",
  "trailingSha256",
  "operationLeg",
]);
const RECEIPT_PAYLOAD_FIELDS = Object.freeze([
  "family",
  "terminal",
  "observedPrefix",
  "authorityPrefix",
  "trailingLength",
  "trailingSha256",
  "selectedLeg",
  "classifiedRoute",
  "proofRoute",
  "outcome",
]);
const RECEIPT_TRANSPORT_FIELDS = Object.freeze([
  "latched",
  "deadlineNs",
  "preNowNs",
  "postNowNs",
  "readResult",
  "sourceState",
  "positiveByteKind",
  "positiveTrailingSha256",
  "lineParseResult",
  "mode",
]);
const RECEIPT_PREFIXES = Object.freeze([
  Object.freeze([]),
  Object.freeze(["P"]),
  Object.freeze(["C"]),
  Object.freeze(["P", "C"]),
  Object.freeze(["C", "P"]),
]);
const RECEIPT_PREFIX_NAMES = Object.freeze([
  "NONE",
  "P",
  "C",
  "P_C",
  "C_P",
]);
const RECEIPT_PREFIX_COUNTS = Object.freeze({
  NONE: 0,
  P: 1,
  C: 1,
  P_C: 2,
  C_P: 2,
});
const RECEIPT_OPERATION_LEGS = Object.freeze([
  "SUPPORT",
  "DENIAL",
  "PARENT_LOSS",
  "ONE_RECEIPT_P_FIRST",
  "ONE_RECEIPT_C_FIRST",
]);
const ORDINARY_RECEIPT_LEGS = Object.freeze(
  RECEIPT_OPERATION_LEGS.slice(0, 3),
);
const ONE_RECEIPT_LEGS = Object.freeze(
  RECEIPT_OPERATION_LEGS.slice(3),
);
const RECEIPT_LINE_RESULTS = Object.freeze([
  "NONE",
  "VALID_P",
  "VALID_C",
  "MALFORMED",
]);
const RECEIPT_MODES = Object.freeze([
  "ORDINARY",
  "ONE_RECEIPT",
]);
const RECEIPT_TERMINAL_FAMILY = Object.freeze({
  CAPACITY_RECORD_193: "CAPACITY",
  GRAMMAR_MALFORMED_OR_DUPLICATE: "GRAMMAR",
  PROTOCOL_THIRD_RECORD: "PROTOCOL",
  PROTOCOL_INTENTIONAL_ONE_RECORD_STOP: "PROTOCOL",
  READ_NON_EAGAIN_ERROR: "READ",
  EOF_TERMINAL: "EOF",
  DEADLINE_NO_EOF: "DEADLINE",
});
const RECEIPT_TERMINAL_LENGTH = Object.freeze({
  CAPACITY_RECORD_193: Object.freeze([192, 192]),
  GRAMMAR_MALFORMED_OR_DUPLICATE: Object.freeze([1, 192]),
  PROTOCOL_THIRD_RECORD: Object.freeze([1, 192]),
  PROTOCOL_INTENTIONAL_ONE_RECORD_STOP: Object.freeze([1, 70]),
  READ_NON_EAGAIN_ERROR: Object.freeze([0, 191]),
  EOF_TERMINAL: Object.freeze([0, 191]),
  DEADLINE_NO_EOF: Object.freeze([0, 191]),
});
const RECEIPT_RED_PROOF_ROUTE = Object.freeze({
  ZERO_EOF: "RECEIPT_NO_ANCHOR_DIRECT_ONLY",
  NON_EAGAIN_READ_ERROR_NO_SEMANTIC_AUTHORITY:
    "RECEIPT_NO_ANCHOR_DIRECT_ONLY",
  DEADLINE_ZERO_COMPLETE_VALID_ROLE:
    "RECEIPT_NO_ANCHOR_DIRECT_ONLY",
  DEADLINE_ONE_COMPLETE_VALID_ROLE:
    "RECEIPT_ANCHORED_EARLY_CLEANUP",
  DEADLINE_TWO_COMPLETE_VALID_ROLES:
    "RECEIPT_ANCHORED_EARLY_CLEANUP",
  PARTIAL_NO_LF_EOF: "RECEIPT_NO_ANCHOR_DIRECT_ONLY",
  ONE_VALID_PLUS_PARTIAL_EOF:
    "RECEIPT_ANCHORED_EARLY_CLEANUP",
  TWO_VALID_PLUS_PARTIAL_EOF:
    "RECEIPT_ANCHORED_EARLY_CLEANUP",
  MALFORMED_DUPLICATE_OR_THIRD: "BY_EARLIER_BOUND_ROLE_COUNT",
  RECORD_OVER_CAP: "BY_EARLIER_BOUND_ROLE_COUNT",
  ORDINARY_ONE_RECORD_EOF: "RECEIPT_ANCHORED_EARLY_CLEANUP",
});
const RECEIPT_CONDITIONAL_PROOF_ROUTE = Object.freeze({
  0: "RECEIPT_NO_ANCHOR_DIRECT_ONLY",
  1: "RECEIPT_ANCHORED_EARLY_CLEANUP",
  2: "RECEIPT_ANCHORED_EARLY_CLEANUP",
});
const RECEIPT_SUCCESS_PROOF_ROUTE = Object.freeze({
  SUPPORT: "support",
  DENIAL: "denial",
  PARENT_LOSS: "parent_loss",
});
const RECEIPT_PARTIAL_EOF_ROUTE = Object.freeze([
  "PARTIAL_NO_LF_EOF",
  "ONE_VALID_PLUS_PARTIAL_EOF",
  "TWO_VALID_PLUS_PARTIAL_EOF",
]);
const RECEIPT_DEADLINE_ROUTE = Object.freeze([
  "DEADLINE_ZERO_COMPLETE_VALID_ROLE",
  "DEADLINE_ONE_COMPLETE_VALID_ROLE",
  "DEADLINE_TWO_COMPLETE_VALID_ROLES",
]);
const CLEANUP_COUNTERPART_ROUTES = Object.freeze([
  "ORDINARY_ONE_RECORD_EOF",
  "ONE_VALID_PLUS_PARTIAL_EOF",
  "DEADLINE_ONE_COMPLETE_VALID_ROLE",
  "MALFORMED_DUPLICATE_OR_THIRD",
  "RECORD_OVER_CAP",
]);
const NO_TRAILING_SHA256 = "NONE";

function receiptDetail(value) {
  if (value === null) return "[null]";
  if (typeof value === "object") return "[object]";
  if (typeof value === "function") return "[function]";
  if (typeof value === "symbol") return "[symbol]";
  if (typeof value === "bigint") return "[bigint]";
  if (value === undefined) return "[undefined]";
  if (typeof value === "string") return value.slice(0, 80);
  if (typeof value === "number") {
    if (Object.is(value, -0)) return "[-0]";
    return Number.isFinite(value) ? String(value) : "[number]";
  }
  return value ? "true" : "false";
}

function requireReceipt(condition, message, detail = undefined) {
  if (!condition) {
    fail(
      "host.ruby_receipt",
      message,
      Object.freeze({ detail: receiptDetail(detail) }),
    );
  }
}

function exactPrimitiveArray(left, right) {
  return (
    left.length === right.length &&
    left.every((value, index) => value === right[index])
  );
}

function receiptPrefixIndex(roles) {
  return RECEIPT_PREFIXES.findIndex((candidate) =>
    exactPrimitiveArray(roles, candidate)
  );
}

function copyReceiptSourceState(sourceState) {
  requireReceipt(
    sourceState !== null &&
      typeof sourceState === "object" &&
      !utilTypes.isProxy(sourceState) &&
      !Array.isArray(sourceState) &&
      Object.getPrototypeOf(sourceState) === Object.prototype,
    "receipt source state must be a plain data object",
    sourceState,
  );
  const descriptors = Object.getOwnPropertyDescriptors(sourceState);
  const keys = Reflect.ownKeys(descriptors);
  requireReceipt(
    keys.every((key) => typeof key === "string") &&
      keys.length === RECEIPT_SOURCE_FIELDS.length &&
      RECEIPT_SOURCE_FIELDS.every((field) => keys.includes(field)),
    "receipt source state has a closed field set",
    keys.length,
  );
  for (const field of RECEIPT_SOURCE_FIELDS) {
    const descriptor = descriptors[field];
    requireReceipt(
      descriptor !== undefined &&
        Object.hasOwn(descriptor, "value") &&
        !Object.hasOwn(descriptor, "get") &&
        !Object.hasOwn(descriptor, "set") &&
        descriptor.enumerable === true,
      "receipt source fields must be enumerable data",
      field,
    );
  }
  const rolesInput = descriptors.roles.value;
  requireReceipt(
    !utilTypes.isProxy(rolesInput) &&
      Array.isArray(rolesInput) &&
      Object.getPrototypeOf(rolesInput) === Array.prototype,
    "receipt roles must be a plain dense array",
    rolesInput,
  );
  const roleDescriptors = Object.getOwnPropertyDescriptors(rolesInput);
  const lengthDescriptor = roleDescriptors.length;
  requireReceipt(
    lengthDescriptor !== undefined &&
      Object.hasOwn(lengthDescriptor, "value") &&
      !Object.hasOwn(lengthDescriptor, "get") &&
      !Object.hasOwn(lengthDescriptor, "set") &&
      lengthDescriptor.enumerable === false,
    "receipt roles length must be plain data",
  );
  const roleLength = lengthDescriptor.value;
  requireReceipt(
    Number.isSafeInteger(roleLength) &&
      roleLength >= 0 &&
      roleLength <= 2,
    "receipt roles length is outside the closed bound",
    roleLength,
  );
  const expectedRoleKeys = [
    ...Array.from({ length: roleLength }, (_, index) => String(index)),
    "length",
  ];
  const roleKeys = Reflect.ownKeys(roleDescriptors);
  requireReceipt(
    roleKeys.every((key) => typeof key === "string") &&
      exactPrimitiveArray(roleKeys, expectedRoleKeys),
    "receipt roles must have dense exact own keys",
    roleKeys.length,
  );
  const roles = [];
  for (let index = 0; index < roleLength; index += 1) {
    const descriptor = roleDescriptors[String(index)];
    requireReceipt(
      descriptor !== undefined &&
        Object.hasOwn(descriptor, "value") &&
        !Object.hasOwn(descriptor, "get") &&
        !Object.hasOwn(descriptor, "set") &&
        descriptor.enumerable === true,
      "receipt role elements must be enumerable data",
      index,
    );
    roles.push(descriptor.value);
  }
  const prefixIndex = receiptPrefixIndex(roles);
  requireReceipt(
    prefixIndex >= 0,
    "receipt roles are not one closed unique prefix",
    roles.length,
  );
  const trailingLength = descriptors.trailingLength.value;
  const trailingSha256 = descriptors.trailingSha256.value;
  const operationLeg = descriptors.operationLeg.value;
  requireReceipt(
    Number.isSafeInteger(trailingLength) &&
      !Object.is(trailingLength, -0) &&
      trailingLength >= 0 &&
      trailingLength <= 192,
    "receipt trailing length is outside the closed bound",
    trailingLength,
  );
  if (trailingLength === 0) {
    requireReceipt(
      trailingSha256 === NO_TRAILING_SHA256,
      "zero receipt trailing length requires NONE hash",
      trailingSha256,
    );
  } else {
    requireReceipt(
      typeof trailingSha256 === "string" &&
        /^[a-f0-9]{64}$/u.test(trailingSha256),
      "positive receipt trailing length requires lowercase SHA-256",
      trailingSha256,
    );
  }
  requireReceipt(
    typeof operationLeg === "string" &&
      RECEIPT_OPERATION_LEGS.includes(operationLeg),
    "receipt operation leg is outside the closed domain",
    operationLeg,
  );
  return Object.freeze({
    roles: Object.freeze(roles),
    observedPrefix: RECEIPT_PREFIX_NAMES[prefixIndex],
    trailingLength,
    trailingSha256,
    operationLeg,
  });
}

function receiptModeForState(mode, copiedState) {
  requireReceipt(
    typeof mode === "string" && RECEIPT_MODES.includes(mode),
    "receipt mode is outside the closed domain",
    mode,
  );
  if (mode === "ONE_RECEIPT") {
    requireReceipt(
      ONE_RECEIPT_LEGS.includes(copiedState.operationLeg) &&
        copiedState.roles.length === 0,
      "unlatched one-receipt state must name its empty selected order",
      copiedState.operationLeg,
    );
  } else {
    requireReceipt(
      ORDINARY_RECEIPT_LEGS.includes(copiedState.operationLeg),
      "ordinary receipt state must name an ordinary leg",
      copiedState.operationLeg,
    );
  }
  return mode;
}

function continuationReceiptSourceState(copiedState, overrides = {}) {
  return Object.freeze({
    roles: Object.freeze([
      ...(overrides.roles === undefined
        ? copiedState.roles
        : overrides.roles),
    ]),
    trailingLength:
      overrides.trailingLength === undefined
        ? copiedState.trailingLength
        : overrides.trailingLength,
    trailingSha256:
      overrides.trailingSha256 === undefined
        ? copiedState.trailingSha256
        : overrides.trailingSha256,
    operationLeg: copiedState.operationLeg,
  });
}

function ordinaryRedProofRoute(route, boundRoleCount) {
  requireReceipt(
    typeof route === "string" &&
      Object.hasOwn(RECEIPT_RED_PROOF_ROUTE, route),
    "ordinary receipt red route is outside the closed domain",
    route,
  );
  const selected = RECEIPT_RED_PROOF_ROUTE[route];
  if (selected !== "BY_EARLIER_BOUND_ROLE_COUNT") return selected;
  requireReceipt(
    route === "MALFORMED_DUPLICATE_OR_THIRD" ||
      route === "RECORD_OVER_CAP",
    "conditional receipt route is outside the closed domain",
    route,
  );
  requireReceipt(
    Number.isSafeInteger(boundRoleCount) &&
      Object.hasOwn(RECEIPT_CONDITIONAL_PROOF_ROUTE, boundRoleCount),
    "conditional receipt role count is outside the closed domain",
    boundRoleCount,
  );
  return RECEIPT_CONDITIONAL_PROOF_ROUTE[boundRoleCount];
}

function ordinarySuccessProofRoute(operationLeg) {
  requireReceipt(
    typeof operationLeg === "string" &&
      Object.hasOwn(RECEIPT_SUCCESS_PROOF_ROUTE, operationLeg),
    "ordinary receipt success leg is outside the closed domain",
    operationLeg,
  );
  return RECEIPT_SUCCESS_PROOF_ROUTE[operationLeg];
}

function classifyReceiptTerminalPayload(terminal, copiedState) {
  requireReceipt(
    typeof terminal === "string" &&
      Object.hasOwn(RECEIPT_TERMINAL_FAMILY, terminal),
    "receipt terminal is outside the closed domain",
    terminal,
  );
  const boundRoleCount =
    RECEIPT_PREFIX_COUNTS[copiedState.observedPrefix];
  requireReceipt(
    Number.isSafeInteger(boundRoleCount),
    "receipt observed prefix is outside the closed domain",
    copiedState.observedPrefix,
  );
  if (ONE_RECEIPT_LEGS.includes(copiedState.operationLeg)) {
    requireReceipt(
      terminal === "PROTOCOL_INTENTIONAL_ONE_RECORD_STOP"
        ? boundRoleCount === 1
        : boundRoleCount === 0,
      "one-receipt terminal has an unreachable role prefix",
      boundRoleCount,
    );
  }
  const lengthDomain = RECEIPT_TERMINAL_LENGTH[terminal];
  requireReceipt(
    copiedState.trailingLength >= lengthDomain[0] &&
      copiedState.trailingLength <= lengthDomain[1],
    "receipt terminal trailing length is incompatible",
    copiedState.trailingLength,
  );
  let authorityPrefix = copiedState.observedPrefix;
  let classifiedRoute;
  let proofRoute;
  let outcome = "TYPED_STOP";
  let selectedLeg = "NONE";

  if (terminal === "CAPACITY_RECORD_193") {
    requireReceipt(
      copiedState.trailingLength === 192,
      "receipt capacity terminal must retain 192 bytes",
    );
    classifiedRoute = "RECORD_OVER_CAP";
    proofRoute = ordinaryRedProofRoute(
      classifiedRoute,
      boundRoleCount,
    );
  } else if (terminal === "GRAMMAR_MALFORMED_OR_DUPLICATE") {
    requireReceipt(
      boundRoleCount <= 1,
      "receipt grammar terminal cannot follow two roles",
      boundRoleCount,
    );
    classifiedRoute = "MALFORMED_DUPLICATE_OR_THIRD";
    proofRoute = ordinaryRedProofRoute(
      classifiedRoute,
      boundRoleCount,
    );
  } else if (terminal === "PROTOCOL_THIRD_RECORD") {
    requireReceipt(
      boundRoleCount === 2,
      "receipt third-record terminal requires two prior roles",
      boundRoleCount,
    );
    classifiedRoute = "MALFORMED_DUPLICATE_OR_THIRD";
    proofRoute = ordinaryRedProofRoute(
      classifiedRoute,
      boundRoleCount,
    );
  } else if (
    terminal === "PROTOCOL_INTENTIONAL_ONE_RECORD_STOP"
  ) {
    const wantedLeg =
      copiedState.observedPrefix === "P"
        ? "ONE_RECEIPT_P_FIRST"
        : copiedState.observedPrefix === "C"
          ? "ONE_RECEIPT_C_FIRST"
          : "NONE";
    requireReceipt(
      wantedLeg !== "NONE" &&
        copiedState.operationLeg === wantedLeg,
      "intentional one-receipt order differs",
      copiedState.operationLeg,
    );
    selectedLeg = wantedLeg;
    classifiedRoute =
      "PARSER_INTENTIONALLY_STOPPED_AFTER_ONE_RECORD";
    proofRoute = "EXISTING_COMPLEX_ONE_RECEIPT_BRANCH";
    outcome = "ONE_RECEIPT_ELIGIBLE_AFTER_CLEANUP";
  } else if (terminal === "READ_NON_EAGAIN_ERROR") {
    authorityPrefix = "NONE";
    classifiedRoute =
      "NON_EAGAIN_READ_ERROR_NO_SEMANTIC_AUTHORITY";
    proofRoute = ordinaryRedProofRoute(classifiedRoute, 0);
  } else if (terminal === "EOF_TERMINAL") {
    if (copiedState.trailingLength > 0) {
      classifiedRoute = RECEIPT_PARTIAL_EOF_ROUTE[boundRoleCount];
      proofRoute = ordinaryRedProofRoute(
        classifiedRoute,
        boundRoleCount,
      );
    } else if (boundRoleCount === 0) {
      classifiedRoute = "ZERO_EOF";
      proofRoute = ordinaryRedProofRoute(classifiedRoute, 0);
    } else if (boundRoleCount === 1) {
      classifiedRoute = "ORDINARY_ONE_RECORD_EOF";
      proofRoute = ordinaryRedProofRoute(classifiedRoute, 1);
    } else {
      requireReceipt(
        ORDINARY_RECEIPT_LEGS.includes(copiedState.operationLeg),
        "two-role EOF must select an ordinary operation leg",
        copiedState.operationLeg,
      );
      selectedLeg = copiedState.operationLeg;
      classifiedRoute = "ORDINARY_SUCCESS";
      proofRoute = ordinarySuccessProofRoute(selectedLeg);
      outcome = "ORDINARY_SUCCESS";
    }
  } else if (terminal === "DEADLINE_NO_EOF") {
    classifiedRoute = RECEIPT_DEADLINE_ROUTE[boundRoleCount];
    proofRoute = ordinaryRedProofRoute(
      classifiedRoute,
      boundRoleCount,
    );
  }

  return {
    family: RECEIPT_TERMINAL_FAMILY[terminal],
    terminal,
    observedPrefix: copiedState.observedPrefix,
    authorityPrefix,
    trailingLength: copiedState.trailingLength,
    trailingSha256: copiedState.trailingSha256,
    selectedLeg,
    classifiedRoute,
    proofRoute,
    outcome,
  };
}

function validateReceiptLatchPayload(payload) {
  requireReceipt(
    payload !== null &&
      typeof payload === "object" &&
      !utilTypes.isProxy(payload) &&
      !Array.isArray(payload) &&
      Object.getPrototypeOf(payload) === Object.prototype,
    "receipt latch must be a plain payload",
    payload,
  );
  const descriptors = Object.getOwnPropertyDescriptors(payload);
  const keys = Reflect.ownKeys(descriptors);
  requireReceipt(
    keys.every((key) => typeof key === "string") &&
      exactPrimitiveArray(keys, RECEIPT_PAYLOAD_FIELDS),
    "receipt latch fields must be exact and ordered",
    keys.length,
  );
  const snapshot = {};
  for (const field of RECEIPT_PAYLOAD_FIELDS) {
    const descriptor = descriptors[field];
    requireReceipt(
      descriptor !== undefined &&
        Object.hasOwn(descriptor, "value") &&
        !Object.hasOwn(descriptor, "get") &&
        !Object.hasOwn(descriptor, "set") &&
        descriptor.enumerable === true,
      "receipt latch fields must be enumerable data",
      field,
    );
    const value = descriptor.value;
    requireReceipt(
      typeof value === "string" ||
        (typeof value === "number" &&
          Number.isSafeInteger(value) &&
          !Object.is(value, -0)),
      "receipt latch fields must be primitive",
      value,
    );
    snapshot[field] = value;
  }
  const prefixIndex = RECEIPT_PREFIX_NAMES.indexOf(
    snapshot.observedPrefix,
  );
  requireReceipt(
    prefixIndex >= 0,
    "receipt latch observed prefix is outside the closed domain",
    snapshot.observedPrefix,
  );
  const operationLeg =
    snapshot.selectedLeg === "NONE"
      ? "SUPPORT"
      : snapshot.selectedLeg;
  const expected = classifyReceiptTerminalPayload(
    snapshot.terminal,
    copyReceiptSourceState({
      roles: [...RECEIPT_PREFIXES[prefixIndex]],
      trailingLength: snapshot.trailingLength,
      trailingSha256: snapshot.trailingSha256,
      operationLeg,
    }),
  );
  requireReceipt(
    RECEIPT_PAYLOAD_FIELDS.every(
      (field) => snapshot[field] === expected[field],
    ),
    "receipt latch fields are not the derived terminal payload",
  );
  return Object.freeze(snapshot);
}

const issuedReceiptLatches = new WeakSet();

function latchReceiptCopiedTerminal(existing, terminal, copiedState) {
  requireReceipt(
    existing === null,
    "receipt first-fault latch may be set only once",
    existing,
  );
  requireReceipt(
    Object.isFrozen(copiedState) &&
      Object.isFrozen(copiedState.roles),
    "receipt copied state must be frozen",
  );
  const payload = classifyReceiptTerminalPayload(
    terminal,
    copiedState,
  );
  validateReceiptLatchPayload(payload);
  const frozen = Object.freeze(payload);
  issuedReceiptLatches.add(frozen);
  return frozen;
}

function latchReceiptTerminal(existing, terminal, sourceState) {
  return latchReceiptCopiedTerminal(
    existing,
    terminal,
    copyReceiptSourceState(sourceState),
  );
}

function receiptStepResult({
  latch,
  nextSourceState,
  reads,
  yields,
  reservationAttempts,
  reservationReleases,
  committedBytes,
}) {
  return Object.freeze({
    latch,
    nextSourceState,
    reads,
    yields,
    reservationAttempts,
    reservationReleases,
    committedBytes,
  });
}

function receiptTransportStep(input) {
  requireReceipt(
    input !== null &&
      typeof input === "object" &&
      !utilTypes.isProxy(input) &&
      !Array.isArray(input) &&
      Object.getPrototypeOf(input) === Object.prototype,
    "receipt transport input must be a plain data object",
    input,
  );
  const latchedDescriptor = Object.getOwnPropertyDescriptor(
    input,
    "latched",
  );
  if (latchedDescriptor !== undefined) {
    requireReceipt(
      Object.hasOwn(latchedDescriptor, "value") &&
        !Object.hasOwn(latchedDescriptor, "get") &&
        !Object.hasOwn(latchedDescriptor, "set") &&
        latchedDescriptor.enumerable === true,
      "receipt transport latch must be enumerable data",
    );
    const existingLatch = latchedDescriptor.value;
    if (existingLatch !== null) {
      requireReceipt(
        typeof existingLatch === "object" &&
          existingLatch !== null &&
          issuedReceiptLatches.has(existingLatch) &&
          Object.isFrozen(existingLatch),
        "receipt existing latch lacks owner provenance",
        existingLatch,
      );
      validateReceiptLatchPayload(existingLatch);
      return receiptStepResult({
        latch: existingLatch,
        nextSourceState: null,
        reads: 0,
        yields: 0,
        reservationAttempts: 0,
        reservationReleases: 0,
        committedBytes: 0,
      });
    }
  }
  const descriptors = Object.getOwnPropertyDescriptors(input);
  const keys = Reflect.ownKeys(descriptors);
  requireReceipt(
    keys.every((key) => typeof key === "string"),
    "receipt transport accepts string fields only",
    keys.length,
  );
  for (const key of keys) {
    const descriptor = descriptors[key];
    requireReceipt(
      RECEIPT_TRANSPORT_FIELDS.includes(key) &&
        descriptor !== undefined &&
        Object.hasOwn(descriptor, "value") &&
        !Object.hasOwn(descriptor, "get") &&
        !Object.hasOwn(descriptor, "set") &&
        descriptor.enumerable === true,
      "receipt transport field is outside the closed data domain",
      key,
    );
  }
  const inputValue = (field, fallback) =>
    descriptors[field] === undefined
      ? fallback
      : descriptors[field].value;
  const deadlineNs = inputValue("deadlineNs", 100n);
  const preNowNs = inputValue("preNowNs", 99n);
  const postNowNs = inputValue("postNowNs", 99n);
  const readResult = inputValue("readResult", undefined);
  const sourceState = inputValue("sourceState", {
    roles: [],
    trailingLength: 0,
    trailingSha256: NO_TRAILING_SHA256,
    operationLeg: "SUPPORT",
  });
  const positiveByteKind = inputValue(
    "positiveByteKind",
    "NON_LF",
  );
  const positiveTrailingSha256 = inputValue(
    "positiveTrailingSha256",
    "a".repeat(64),
  );
  const lineParseResult = inputValue("lineParseResult", "NONE");
  const mode = inputValue("mode", "ORDINARY");
  const copiedBeforeRead = copyReceiptSourceState(sourceState);
  receiptModeForState(mode, copiedBeforeRead);
  requireReceipt(
    copiedBeforeRead.trailingLength < 192,
    "unlatched receipt record length 192 is unrepresentable",
    copiedBeforeRead.trailingLength,
  );
  requireReceipt(
    typeof deadlineNs === "bigint" &&
      typeof preNowNs === "bigint" &&
      typeof postNowNs === "bigint" &&
      deadlineNs >= 0n &&
      preNowNs >= 0n &&
      postNowNs >= 0n &&
      preNowNs <= postNowNs,
    "receipt monotonic samples are invalid",
  );
  if (preNowNs > deadlineNs) {
    return receiptStepResult({
      latch: latchReceiptCopiedTerminal(
        null,
        "DEADLINE_NO_EOF",
        copiedBeforeRead,
      ),
      nextSourceState: null,
      reads: 0,
      yields: 0,
      reservationAttempts: 0,
      reservationReleases: 0,
      committedBytes: 0,
    });
  }
  const reads = 1;
  if (postNowNs > deadlineNs) {
    return receiptStepResult({
      latch: latchReceiptCopiedTerminal(
        null,
        "DEADLINE_NO_EOF",
        copiedBeforeRead,
      ),
      nextSourceState: null,
      reads,
      yields: 0,
      reservationAttempts: 1,
      reservationReleases: 1,
      committedBytes: 0,
    });
  }
  if (readResult === "EAGAIN") {
    return receiptStepResult({
      latch: null,
      nextSourceState:
        continuationReceiptSourceState(copiedBeforeRead),
      reads,
      yields: 1,
      reservationAttempts: 1,
      reservationReleases: 1,
      committedBytes: 0,
    });
  }
  if (readResult === "NON_EAGAIN_READ_ERROR") {
    return receiptStepResult({
      latch: latchReceiptCopiedTerminal(
        null,
        "READ_NON_EAGAIN_ERROR",
        copiedBeforeRead,
      ),
      nextSourceState: null,
      reads,
      yields: 0,
      reservationAttempts: 1,
      reservationReleases: 1,
      committedBytes: 0,
    });
  }
  if (readResult === "EOF") {
    return receiptStepResult({
      latch: latchReceiptCopiedTerminal(
        null,
        "EOF_TERMINAL",
        copiedBeforeRead,
      ),
      nextSourceState: null,
      reads,
      yields: 0,
      reservationAttempts: 1,
      reservationReleases: 1,
      committedBytes: 0,
    });
  }
  requireReceipt(
    readResult === "POSITIVE_DATA",
    "receipt read result is outside the closed domain",
    readResult,
  );
  requireReceipt(
    positiveByteKind === "NON_LF" ||
      positiveByteKind === "LF",
    "receipt positive byte kind is outside the closed domain",
    positiveByteKind,
  );
  requireReceipt(
    typeof lineParseResult === "string" &&
      RECEIPT_LINE_RESULTS.includes(lineParseResult) &&
      (positiveByteKind === "NON_LF"
        ? lineParseResult === "NONE"
        : copiedBeforeRead.roles.length === 2 ||
          lineParseResult !== "NONE"),
    "receipt line parse result is incompatible with the byte",
    lineParseResult,
  );
  requireReceipt(
    typeof positiveTrailingSha256 === "string" &&
      /^[a-f0-9]{64}$/u.test(positiveTrailingSha256),
    "receipt committed byte hash is outside the closed domain",
    positiveTrailingSha256,
  );
  const committedSourceState = {
    roles: [...copiedBeforeRead.roles],
    trailingLength: copiedBeforeRead.trailingLength + 1,
    trailingSha256: positiveTrailingSha256,
    operationLeg: copiedBeforeRead.operationLeg,
  };
  if (
    committedSourceState.trailingLength === 192 &&
    positiveByteKind === "NON_LF"
  ) {
    return receiptStepResult({
      latch: latchReceiptTerminal(
        null,
        "CAPACITY_RECORD_193",
        committedSourceState,
      ),
      nextSourceState: null,
      reads,
      yields: 0,
      reservationAttempts: 1,
      reservationReleases: 0,
      committedBytes: 1,
    });
  }
  if (positiveByteKind === "NON_LF") {
    return receiptStepResult({
      latch: null,
      nextSourceState: continuationReceiptSourceState(
        copyReceiptSourceState(committedSourceState),
      ),
      reads,
      yields: 0,
      reservationAttempts: 1,
      reservationReleases: 0,
      committedBytes: 1,
    });
  }

  let terminal = null;
  if (copiedBeforeRead.roles.length === 2) {
    terminal = "PROTOCOL_THIRD_RECORD";
  } else {
    if (committedSourceState.trailingLength > 70) {
      requireReceipt(
        lineParseResult === "MALFORMED",
        "over-length receipt line cannot report a valid role",
        lineParseResult,
      );
    }
    if (lineParseResult === "MALFORMED") {
      terminal = "GRAMMAR_MALFORMED_OR_DUPLICATE";
    }
  }
  if (terminal === null) {
    const parsedRole =
      lineParseResult === "VALID_P" ? "P" : "C";
    if (copiedBeforeRead.roles.includes(parsedRole)) {
      terminal = "GRAMMAR_MALFORMED_OR_DUPLICATE";
    } else if (mode === "ONE_RECEIPT") {
      const wantedLeg =
        parsedRole === "P"
          ? "ONE_RECEIPT_P_FIRST"
          : "ONE_RECEIPT_C_FIRST";
      requireReceipt(
        copiedBeforeRead.roles.length === 0 &&
          copiedBeforeRead.operationLeg === wantedLeg,
        "one-receipt parsed role differs from selected order",
        parsedRole,
      );
      terminal = "PROTOCOL_INTENTIONAL_ONE_RECORD_STOP";
      committedSourceState.roles.push(parsedRole);
    } else {
      return receiptStepResult({
        latch: null,
        nextSourceState: continuationReceiptSourceState(
          copiedBeforeRead,
          {
            roles: [...copiedBeforeRead.roles, parsedRole],
            trailingLength: 0,
            trailingSha256: NO_TRAILING_SHA256,
          },
        ),
        reads,
        yields: 0,
        reservationAttempts: 1,
        reservationReleases: 0,
        committedBytes: 1,
      });
    }
  }
  return receiptStepResult({
    latch: latchReceiptTerminal(
      null,
      terminal,
      committedSourceState,
    ),
    nextSourceState: null,
    reads,
    yields: 0,
    reservationAttempts: 1,
    reservationReleases: 0,
    committedBytes: 1,
  });
}

function parseRubyReceiptCandidate(bytes) {
  if (
    !Buffer.isBuffer(bytes) ||
    bytes.length < 1 ||
    bytes.length > 70 ||
    bytes.at(-1) !== 0x0a ||
    bytes.subarray(0, -1).some(
      (byte) => byte < 0x20 || byte > 0x7e,
    )
  ) {
    return Object.freeze({ result: "MALFORMED", record: null });
  }
  try {
    const record = parseRubyReceiptLine(bytes.toString("ascii"));
    return Object.freeze({
      result: record.role === "P" ? "VALID_P" : "VALID_C",
      record,
    });
  } catch {
    return Object.freeze({ result: "MALFORMED", record: null });
  }
}

function receiptRecordsForAuthority(candidates, authorityPrefix) {
  if (authorityPrefix === "NONE") return Object.freeze([]);
  const wantedRoles = RECEIPT_PREFIXES[
    RECEIPT_PREFIX_NAMES.indexOf(authorityPrefix)
  ];
  requireReceipt(
    wantedRoles !== undefined &&
      candidates.length >= wantedRoles.length,
    "receipt authority prefix has no matching candidates",
    authorityPrefix,
  );
  const selected = candidates.slice(0, wantedRoles.length);
  requireReceipt(
    selected.every(
      (record, index) => record.role === wantedRoles[index],
    ),
    "receipt authority candidates differ from the latched prefix",
    authorityPrefix,
  );
  return Object.freeze(selected);
}

function readReceiptByteOwned({
  fd,
  deadlineNs,
  counters,
  readOne = readSync,
  sampleNow = () => process.hrtime.bigint(),
  allocateOne = () => Buffer.allocUnsafe(1),
}) {
  const preNowNs = sampleNow();
  let postNowNs = preNowNs;
  let readResult;
  let positiveByte;
  let nonEagainReadError = false;
  if (preNowNs > deadlineNs) {
    return Object.freeze({
      preNowNs,
      postNowNs,
      readResult,
      positiveByte,
      nonEagainReadError,
    });
  }

  counters.reservationAttempts += 1;
  const scratch = allocateOne();
  let count;
  let readError;
  try {
    count = readOne(fd, scratch, 0, 1, null);
  } catch (error) {
    readError = error;
  }
  postNowNs = sampleNow();
  counters.reads += 1;

  if (postNowNs < preNowNs || postNowNs > deadlineNs) {
    counters.reservationReleases += 1;
    return Object.freeze({
      preNowNs,
      postNowNs,
      readResult,
      positiveByte,
      nonEagainReadError,
    });
  }

  if (readError === undefined) {
    if (count !== 0 && count !== 1) {
      counters.reservationReleases += 1;
      requireReceipt(
        false,
        "Ruby receipt one-byte read returned an impossible count",
        count,
      );
    }
    if (count === 0) {
      readResult = "EOF";
      counters.reservationReleases += 1;
    } else {
      readResult = "POSITIVE_DATA";
      counters.committedBytes += 1;
      positiveByte = Buffer.from(scratch);
    }
  } else if (readError?.code === "EAGAIN") {
    readResult = "EAGAIN";
    counters.reservationReleases += 1;
  } else {
    readResult = "NON_EAGAIN_READ_ERROR";
    nonEagainReadError = true;
    counters.reservationReleases += 1;
  }

  return Object.freeze({
    preNowNs,
    postNowNs,
    readResult,
    positiveByte,
    nonEagainReadError,
  });
}

function rubyReceiptReader(
  fd,
  deadline,
  mode,
  ordinaryOperationLeg,
) {
  requireReceipt(
    RECEIPT_MODES.includes(mode) &&
      (mode === "ORDINARY"
        ? ORDINARY_RECEIPT_LEGS.includes(ordinaryOperationLeg)
        : ordinaryOperationLeg === null),
    "Ruby receipt reader mode/operation differs",
    mode,
  );
  let closed = false;
  const close = () => {
    if (!closed) {
      closeSync(fd);
      closed = true;
    }
  };
  const promise = (async () => {
    let sourceState = Object.freeze({
      roles: Object.freeze([]),
      trailingLength: 0,
      trailingSha256: NO_TRAILING_SHA256,
      operationLeg:
        mode === "ORDINARY"
          ? ordinaryOperationLeg
          : "ONE_RECEIPT_P_FIRST",
    });
    let currentRecord = Buffer.alloc(0);
    const aggregateHash = createHash("sha256");
    const aggregateChunks = [];
    let aggregateBytes = 0;
    const candidates = [];
    const counters = {
      reads: 0,
      yields: 0,
      reservationAttempts: 0,
      reservationReleases: 0,
      committedBytes: 0,
    };
    let nonEagainReadError = false;
    while (true) {
      const countersBefore = Object.freeze({ ...counters });
      let preNowNs;
      let postNowNs;
      let readResult;
      let positiveByte;
      let positiveByteKind;
      let positiveTrailingSha256;
      let lineParseResult = "NONE";
      let parsedCandidate = null;
      requireReceipt(
        sourceState.trailingLength < 192 &&
          aggregateBytes < 384,
        "Ruby receipt one-byte capacity was not available before read",
        sourceState.trailingLength,
      );
      const ownedRead = readReceiptByteOwned({
        fd,
        deadlineNs: deadline.endsNs,
        counters,
      });
      preNowNs = ownedRead.preNowNs;
      postNowNs = ownedRead.postNowNs;
      readResult = ownedRead.readResult;
      positiveByte = ownedRead.positiveByte;
      nonEagainReadError ||= ownedRead.nonEagainReadError;
      if (
        readResult === "POSITIVE_DATA" &&
        postNowNs <= deadline.endsNs
      ) {
        const nextRecord = Buffer.concat([
          currentRecord,
          positiveByte,
        ]);
        aggregateHash.update(positiveByte);
        aggregateChunks.push(positiveByte);
        aggregateBytes += 1;
        currentRecord = nextRecord;
        requireReceipt(
          aggregateBytes <= 384,
          "Ruby receipt aggregate private guard exceeded",
          aggregateBytes,
        );
        positiveByteKind =
          positiveByte[0] === 0x0a ? "LF" : "NON_LF";
        positiveTrailingSha256 = sha256(nextRecord);
        if (
          positiveByteKind === "LF" &&
          sourceState.roles.length < 2
        ) {
          parsedCandidate = parseRubyReceiptCandidate(nextRecord);
          lineParseResult = parsedCandidate.result;
          if (
            mode === "ONE_RECEIPT" &&
            (lineParseResult === "VALID_P" ||
              lineParseResult === "VALID_C")
          ) {
            sourceState = Object.freeze({
              roles: sourceState.roles,
              trailingLength: sourceState.trailingLength,
              trailingSha256: sourceState.trailingSha256,
              operationLeg:
                lineParseResult === "VALID_P"
                  ? "ONE_RECEIPT_P_FIRST"
                  : "ONE_RECEIPT_C_FIRST",
            });
          }
        }
      }
      const step = receiptTransportStep({
        deadlineNs: deadline.endsNs,
        preNowNs,
        postNowNs,
        readResult,
        sourceState,
        positiveByteKind,
        positiveTrailingSha256,
        lineParseResult,
        mode,
      });
      counters.yields += step.yields;
      requireReceipt(
        counters.reads - countersBefore.reads === step.reads &&
          counters.yields - countersBefore.yields === step.yields &&
          counters.reservationAttempts -
            countersBefore.reservationAttempts ===
            step.reservationAttempts &&
          counters.reservationReleases -
            countersBefore.reservationReleases ===
            step.reservationReleases &&
          counters.committedBytes -
            countersBefore.committedBytes ===
            step.committedBytes,
        "Ruby receipt live charge differs from the pure transition",
        step.latch?.terminal ?? "CONTINUE",
      );
      if (step.committedBytes === 1) {
        if (positiveByteKind === "LF") {
          if (
            parsedCandidate !== null &&
            parsedCandidate.record !== null &&
            (step.nextSourceState !== null ||
              step.latch?.terminal ===
                "PROTOCOL_INTENTIONAL_ONE_RECORD_STOP")
          ) {
            candidates.push(parsedCandidate.record);
          }
          currentRecord = Buffer.alloc(0);
        }
      }
      if (step.latch !== null) {
        currentRecord = Buffer.alloc(0);
        const records = receiptRecordsForAuthority(
          candidates,
          step.latch.authorityPrefix,
        );
        return Object.freeze({
          bytes: aggregateBytes,
          sha256: aggregateHash.digest("hex"),
          body: Buffer.concat(
            aggregateChunks,
            aggregateBytes,
          ),
          eof: step.latch.terminal === "EOF_TERMINAL",
          readerRetained:
            step.latch.terminal !== "EOF_TERMINAL",
          nonEagainReadError,
          latch: step.latch,
          records,
          counters: Object.freeze({ ...counters }),
        });
      }
      sourceState = step.nextSourceState;
      if (step.yields === 1) {
        await new Promise((resolvePromise) => {
          setImmediate(resolvePromise);
        });
      }
    }
  })();
  return Object.freeze({
    promise,
    close,
    isClosed() {
      return closed;
    },
  });
}

function createRubyRoots(worker, containerReceipt, previous) {
  const receipts = {};
  for (const role of ["home", "tmp", "cwd", "parent", "child"]) {
    const path = join(containerReceipt.path, role);
    const receipt = createDirectory(
      path,
      `ruby-${role}`,
      worker.capacity,
    );
    if (previous !== undefined) {
      requireCondition(
        receipt.dev !== previous[role].dev ||
          receipt.ino !== previous[role].ino,
        "host.root_identity",
        `Ruby ${role} directory inode was reused`,
      );
    }
    receipts[role] = receipt;
  }
  const parentPath = join(receipts.parent.path, "control.sock");
  const childPath = join(receipts.child.path, "control.sock");
  requireCondition(
    Buffer.byteLength(parentPath) === 76 &&
      Buffer.byteLength(childPath) === 75 &&
      parentPath !== childPath,
    "host.ruby_path",
    "Ruby socket path spelling differs",
    {
      parentBytes: Buffer.byteLength(parentPath),
      childBytes: Buffer.byteLength(childPath),
    },
  );
  assertAbsentTwice(parentPath);
  assertAbsentTwice(childPath);
  return Object.freeze({
    receipts: Object.freeze(receipts),
    parentPath,
    childPath,
  });
}

function validateRubyReceiptTopology(state, records, expectedBranch) {
  const parent = records.find((record) => record.role === "P");
  const child = records.find((record) => record.role === "C");
  requireCondition(
    parent !== undefined &&
      child !== undefined &&
      parent.branch === expectedBranch &&
      child.branch === expectedBranch &&
      parent.pid === state.launchPid &&
      parent.ppid === process.pid &&
      parent.pgid === state.launchPid &&
      parent.sid === state.launchPid &&
      child.pid !== parent.pid &&
      child.ppid === parent.pid &&
      child.pgid === state.launchPid &&
      child.sid === state.launchPid,
    "host.ruby_topology",
    "Ruby receipt topology differs",
    { records, launchPid: state.launchPid, workerPid: process.pid },
  );
  if (expectedBranch === "S") {
    for (const [record, path] of [
      [parent, state.roots.parentPath],
      [child, state.roots.childPath],
    ]) {
      const first = statFact(lstatBig(path));
      const second = statFact(lstatBig(path));
      requireCondition(
        first.type === "socket" &&
          first.uid === HOST_UID &&
          first.dev === record.dev &&
          first.ino === record.ino &&
          sameFact(first, second),
        "host.ruby_socket",
        "Ruby socket pathname identity differs from receipt",
        { role: record.role, first, record },
      );
    }
  } else {
    requireCondition(
      absentNoFollow(state.roots.parentPath) &&
        absentNoFollow(state.roots.childPath),
      "host.ruby_denial",
      "Ruby denial created a pathname",
    );
  }
  return Object.freeze({ parent, child });
}

function parsePsRows(body) {
  requireCondition(
    body.length > 0 &&
      body.at(-1) === 0x0a &&
      body.every(
        (byte) =>
          byte === 0x0a || (byte >= 0x20 && byte <= 0x7e),
      ),
    "host.ps_output",
    "ps live output framing differs",
    { bytes: body.length, sha256: sha256(body) },
  );
  const lines = body.toString("ascii").slice(0, -1).split("\n");
  const rows = lines.map((line) => {
    const match =
      /^ *([1-9][0-9]*) +([1-9][0-9]*) +([1-9][0-9]*) +0 +([0-9]+) +([IRSTUZ]s?) +((?:Sun|Mon|Tue|Wed|Thu|Fri|Sat) (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) (?: [1-9]|[12][0-9]|3[01]) (?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9] [0-9]{4}) +ruby {12}$/u.exec(
        line,
      );
    requireCondition(
      match !== null,
      "host.ps_output",
      "ps row grammar differs",
      { bytes: Buffer.byteLength(line), sha256: sha256(Buffer.from(line)) },
    );
    return Object.freeze({
      pid: parsePid(match[1]),
      ppid: parseCanonicalSafeInteger(
        match[2],
        1,
        PID_MAX,
        "ppid",
      ),
      pgid: parsePid(match[3]),
      sessObservedZero: "0",
      uid: parseCanonicalSafeInteger(match[4], 0, 65_535, "uid"),
      state: match[5],
      lstart: match[6],
      ucomm: "ruby",
    });
  });
  requireCondition(
    rows.length >= 1 &&
      rows.length <= 2 &&
      rows.every(
        (row, index) =>
          row.uid === HOST_UID &&
          (index === 0 || row.pid > rows[index - 1].pid),
      ) &&
      new Set(rows.map((row) => row.pid)).size === rows.length,
    "host.ps_output",
    "ps rows are unsorted, duplicate, or outside the live bound",
    { rows },
  );
  return Object.freeze(rows);
}

function stableProcessFact(row) {
  return Object.freeze({
    pid: row.pid,
    pgid: row.pgid,
    sessObservedZero: row.sessObservedZero,
    uid: row.uid,
    lstart: row.lstart,
    ucomm: row.ucomm,
  });
}

function statePrimary(state) {
  return state.state[0];
}

function isStopped(row) {
  return statePrimary(row) === "T";
}

function isZombie(row) {
  return statePrimary(row) === "Z";
}

const consumedProofs = new WeakSet();
const issuedRubyScopes = new WeakSet();
const issuedRubyPositiveTargets = new WeakSet();

function issueRubyScope(launchPid, receiptRecords) {
  requireCondition(
    Number.isSafeInteger(launchPid) &&
      receiptRecords.length >= 1 &&
      receiptRecords.length <= 2 &&
      receiptRecords.every(
        (record) =>
          record.pgid === launchPid &&
          record.sid === launchPid,
      ),
    "host.ruby_scope",
    "valid receipt records did not establish a launch scope",
  );
  const scope = Object.freeze({
    kind: "ReceiptAnchoredRubyScope",
    launchPid,
    pgid: launchPid,
    anchorRole: receiptRecords[0].role,
    anchorPid: receiptRecords[0].pid,
  });
  issuedRubyScopes.add(scope);
  return scope;
}

function issueReceiptBoundRoleFact(record, row) {
  requireCondition(
    record.pid === row.pid &&
      record.pgid === row.pgid &&
      row.sessObservedZero === "0" &&
      row.uid === HOST_UID &&
      row.ucomm === "ruby",
    "host.ruby_identity",
    "receipt and ps facts cannot bind one Ruby role",
  );
  const fact = Object.freeze({
    kind: "ReceiptBoundRubyRoleFact",
    role: record.role,
    pid: record.pid,
    receiptPpid: record.ppid,
    pgid: record.pgid,
    sid: record.sid,
    uid: row.uid,
    lstart: row.lstart,
    ucomm: row.ucomm,
    receiptDigest: sha256(Buffer.from(canonicalJson(record))),
  });
  issuedRubyPositiveTargets.add(fact);
  return fact;
}

function issueObservedRubyCounterpart(kind, role, row, state) {
  requireCondition(
    (kind === "OneReceiptProvisionalRubyRole" ||
      kind === "CleanupBoundRubyCounterpart") &&
      (role === "P" || role === "C") &&
      row.pgid === state.launchPid &&
      row.sessObservedZero === "0" &&
      row.uid === HOST_UID &&
      row.ucomm === "ruby",
    "host.ruby_identity",
    "ps row cannot project the requested Ruby counterpart",
  );
  const fact = Object.freeze({
    kind,
    role,
    pid: row.pid,
    pgid: row.pgid,
    sessObservedZero: row.sessObservedZero,
    uid: row.uid,
    lstart: row.lstart,
    ucomm: row.ucomm,
  });
  requireCondition(
    !Object.hasOwn(fact, "sid") &&
      !Object.hasOwn(fact, "receiptDigest"),
    "host.ruby_identity",
    "ps-only counterpart improperly contains receipt authority",
  );
  issuedRubyPositiveTargets.add(fact);
  return fact;
}

function rubyFactMatchesRow(fact, row) {
  return (
    fact.pid === row.pid &&
    fact.pgid === row.pgid &&
    fact.uid === row.uid &&
    fact.lstart === row.lstart &&
    fact.ucomm === row.ucomm &&
    row.sessObservedZero === "0"
  );
}

const RUBY_CAPTURE_ACTIONS = Object.freeze([
  "NONE",
  "GROUP_CONT",
  "GROUP_KILL",
  "GROUP_KILL_IF_PRESENT",
  "PARENT_KILL",
  "SURVIVOR_STOP",
  "SURVIVOR_CONT",
  "SURVIVOR_TERM",
  "SURVIVOR_KILL",
]);

function consumeFreshRubyObservation(
  owner,
  state,
  ordinal,
  predicate,
  label,
  captureAction,
  launched,
  settled,
  stdoutResult,
  stderrResult,
  rows,
) {
  owner.bindRows(state, rows, ordinal);
  requireCondition(
    predicate(rows),
    "host.proof_predicate",
    `Ruby ${label} predicate differs`,
    { leg: state.leg, ordinal, rows },
  );
  const proof = Object.freeze({
    leg: state.leg,
    ordinal,
    label,
    launchPid: state.launchPid,
    rows,
    capturePid: launched.pid,
    terminal: settled.terminal,
    stdout: captureFacts(stdoutResult),
    stderr: captureFacts(stderrResult),
    monotonicNs: process.hrtime.bigint().toString(),
  });
  const transition = rubyCaptureTransition(
    captureAction,
    state,
    proof,
  );
  let actionRecord = null;
  if (transition.signal !== null) {
    consumedProofs.add(proof);
    fail(
      "host.legacy_numeric_signal_forbidden",
      "legacy Node-side Ruby signal projection is disabled",
      {
        transition: transition.kind,
        signal: transition.signal,
      },
    );
  } else if (transition.consume) {
    consumedProofs.add(proof);
    if (transition.alreadyStopped) {
      actionRecord = Object.freeze({
        kind: "ruby.already_stopped",
        facts: Object.freeze({
          leg: state.leg,
          ordinal: proof.ordinal,
          pid: proof.rows[0].pid,
          proofSha256: sha256(Buffer.from(canonicalJson(proof))),
        }),
      });
    }
  }
  state.consumed.add(ordinal);
  state.lastOrdinal = ordinal;
  state.proofRows.set(ordinal, rows);
  state.proofs.set(ordinal, proof);
  if (actionRecord !== null) {
    state.actions.set(ordinal, actionRecord);
  }
  return Object.freeze({ proof, actionRecord });
}

function receiptFaultCustodyReceipt(state, proof, action) {
  fail(
    "host.legacy_numeric_signal_forbidden",
    "legacy receipt-fault signal custody is disabled",
    {
      leg: state?.leg ?? null,
      proofOrdinal: proof?.ordinal ?? null,
      actionPresent: action !== null && action !== undefined,
    },
  );
}

class RubyProofOwner {
  constructor(worker, fifo, evidence, tombstones, canary) {
    this.worker = worker;
    this.fifo = fifo;
    this.evidence = evidence;
    this.tombstones = tombstones;
    this.canary = canary;
    this.nextBatch = 0;
    this.absenceOraclePromoted = false;
    this.actionByProof = new WeakMap();
  }

  async capture(
    state,
    ordinal,
    predicate,
    label,
    captureAction = "NONE",
  ) {
    requireCondition(
      state.branch.includes(ordinal) &&
        !state.consumed.has(ordinal) &&
        ordinal > state.lastOrdinal &&
        ordinal >= 1 &&
        ordinal <= 14 &&
        RUBY_CAPTURE_ACTIONS.includes(captureAction),
      "host.proof_ordinal",
      "Ruby proof ordinal/action is not available on the selected branch",
      {
        leg: state.leg,
        ordinal,
        branch: state.branch,
        captureAction,
      },
    );
    const reservations = reserveS2CaptureAttempt(
      this.worker.capacity,
    );
    let proofReservation;
    checkCanary(this.canary, state.deadline);
    requireCondition(
      this.nextBatch < LIMITS.psCaptures,
      "host.proof_capacity",
      "Ruby proof/ps capacity exceeded",
      {
        reservedProofs:
          this.worker.capacity.reserved.proofs,
        reservedPs:
          this.worker.capacity.reserved.psCaptures,
        nextBatch: this.nextBatch,
      },
    );
    const proofDeadline = state.deadline.sub(
      `proof-${String(ordinal).padStart(2, "0")}`,
      DEADLINE_MS.ps,
    );
    const batchIndex = this.nextBatch;
    this.nextBatch += 1;
    const batch = await this.fifo.create(
      batchIndex,
      ["stdout.fifo", "stderr.fifo"],
      proofDeadline.sub("fifo", DEADLINE_MS.fifoBatch),
    );
    const stdout = channelReader(
      batch.endpoints[0].reader,
      "ps stdout",
      4_096,
      proofDeadline,
    );
    const stderr = channelReader(
      batch.endpoints[1].reader,
      "ps stderr",
      512,
      proofDeadline,
    );
    const stdin = openDevNull(fsConstants.O_RDONLY, proofDeadline);
    let launched;
    try {
      if (state.leg === "one-receipt" &&
          state.firstRole === "P" &&
          ordinal === 1) {
        requireProtocolPeak(33);
      } else {
        requireProtocolPeak(32);
      }
      launched = spawnExact({
        executable: PS,
        args: [
          "-ww",
          "-g",
          String(state.launchPid),
          "-o",
          "pid=,ppid=,pgid=,sess=,uid=,state=,lstart=,ucomm=",
        ],
        cwd: batch.batchPath,
        env: closedEnvironment(
          state.roots.receipts.home.path,
          state.roots.receipts.tmp.path,
        ),
        stdio: [
          stdin.fd,
          batch.endpoints[0].writer,
          batch.endpoints[1].writer,
        ],
        label: `${state.leg}-ps-${String(ordinal).padStart(2, "0")}`,
        tombstones: this.tombstones,
      });
    } finally {
      checkedClose(stdin.fd, proofDeadline);
    }
    closeHandoff(batch);
    const [settled, stdoutResult, stderrResult] = await Promise.all([
      settleDirectChild(launched, proofDeadline, {
        normalMs: 1_000,
        termMs: 250,
        killMs: 250,
        label: launched.label,
      }),
      stdout.promise,
      stderr.promise,
    ]);
    requireCondition(
      settled.terminal.error === null &&
        settled.terminal.signal === null &&
        stderrResult.eof &&
        stderrResult.bytes === 0 &&
        stdoutResult.eof,
      "host.ps_output",
      "ps status/EOF/stderr differs",
      {
        terminal: settled.terminal,
        stdout: captureFacts(stdoutResult),
        stderr: captureFacts(stderrResult),
      },
    );
    let rows;
    if (settled.terminal.code === 0) {
      rows = parsePsRows(stdoutResult.body);
    } else {
      requireCondition(
        settled.terminal.code === 1 &&
          stdoutResult.bytes === 0 &&
          stderrResult.bytes === 0,
        "host.ps_absence",
        "ps absence oracle status/output differs",
        {
          terminal: settled.terminal,
          stdout: captureFacts(stdoutResult),
          stderr: captureFacts(stderrResult),
        },
      );
      rows = Object.freeze([]);
      if (state.leg === "support" && ordinal === 13) {
        this.absenceOraclePromoted = true;
      } else {
        requireCondition(
          this.absenceOraclePromoted || state.cleanupMode === true,
          "host.ps_absence",
          "ps absence was consumed before the red-first oracle",
          { leg: state.leg, ordinal },
        );
      }
    }
    let observation;
    let observationFault;
    try {
      observation = consumeFreshRubyObservation(
        this,
        state,
        ordinal,
        predicate,
        label,
        captureAction,
        launched,
        settled,
        stdoutResult,
        stderrResult,
        rows,
      );
    } catch (error) {
      observationFault =
        error instanceof HostAuthorityError
          ? error
          : new HostAuthorityError(
              "host.proof_observation",
              "Ruby fresh observation projection failed",
              safeError(error),
            );
    }
    let proofRetirementFault;
    try {
      this.fifo.retire(batch, proofDeadline);
      this.tombstones.add(launched.pid, "PS_DIRECT_REAP");
    } catch (error) {
      proofRetirementFault = error;
    }
    const proofFailure = aggregate(
      observationFault,
      [proofRetirementFault],
    );
    if (proofFailure !== undefined) throw proofFailure;
    const { proof, actionRecord } = observation;
    this.actionByProof.set(proof, actionRecord);
    this.evidence.add(
      "ps",
      "ruby.proof",
      proof,
      proofDeadline,
    );
    if (actionRecord !== null) {
      this.evidence.add(
        "transitions",
        actionRecord.kind,
        actionRecord.facts,
        state.deadline,
      );
    }
    this.worker.capacity.complete(reservations.proof);
    this.worker.capacity.complete(reservations.ps);
    return proof;
  }

  async captureReceiptFaultCustody(
    state,
    predicate,
    label,
  ) {
    let proof;
    let deferredFault;
    try {
      proof = await this.capture(
        state,
        12,
        predicate,
        label,
        "GROUP_KILL",
      );
    } catch (error) {
      proof = state.proofs.get(12);
      deferredFault = error;
    }
    const action =
      proof === undefined
        ? undefined
        : state.actions.get(12) ??
          this.actionByProof.get(proof);
    requireCondition(
      proof !== undefined && action !== undefined,
      "host.ruby_receipt_custody",
      "anchored capture failed before typed signal custody",
      {
        leg: state.leg,
        lastOrdinal: state.lastOrdinal,
        captureFault:
          deferredFault === undefined
            ? null
            : safeError(deferredFault),
      },
    );
    this.actionByProof.delete(proof);
    return Object.freeze({
      custody: receiptFaultCustodyReceipt(
        state,
        proof,
        action,
      ),
      deferredFault,
    });
  }

  bindRows(state, rows, ordinal) {
    const postParentReap = state.parentReaped && ordinal >= 5;
    const earlyReceiptCleanup =
      !state.parentReaped &&
      ordinal === 12 &&
      state.receiptProofRoute ===
        "RECEIPT_ANCHORED_EARLY_CLEANUP";
    requireCondition(
      !(
        ordinal >= 5 &&
        !state.parentReaped &&
        !earlyReceiptCleanup &&
        state.cleanupMode !== true &&
        rows.length > 0
      ),
      "host.ruby_topology",
      "post-parent ordinal observed rows before direct-parent reap",
      { leg: state.leg, ordinal },
    );
    for (const row of rows) {
      requireCondition(
        row.pgid === state.launchPid &&
          row.sessObservedZero === "0" &&
          row.ucomm === "ruby" &&
          row.uid === HOST_UID,
        "host.ruby_topology",
        "ps row is outside the anchored Ruby group/session",
        row,
      );
      const receipt = state.receiptByPid.get(row.pid);
      let fact = state.roleFactsByPid.get(row.pid);
      if (receipt !== undefined && fact === undefined) {
        requireCondition(
          !postParentReap &&
            ((receipt.role === "P" &&
              row.pid === state.launchPid &&
              row.ppid === process.pid) ||
              (receipt.role === "C" &&
                row.pid !== state.launchPid &&
                row.ppid === state.launchPid)),
          "host.ruby_topology",
          "first receipt-bound role observation is not launch-rooted",
          { leg: state.leg, ordinal, role: receipt.role },
        );
        fact = issueReceiptBoundRoleFact(receipt, row);
        state.roleFactsByPid.set(row.pid, fact);
      } else if (receipt === undefined && fact === undefined) {
        const intentionalAcquisition =
          state.intentionalOneReceipt &&
          ordinal === 1 &&
          state.receiptByPid.size === 1 &&
          state.provisionalByPid.size === 0 &&
          state.cleanupByPid.size === 0;
        const cleanupAcquisition =
          !state.intentionalOneReceipt &&
          ordinal === 12 &&
          state.receiptProofRoute ===
            "RECEIPT_ANCHORED_EARLY_CLEANUP" &&
          CLEANUP_COUNTERPART_ROUTES.includes(
            state.receiptClassifiedRoute,
          ) &&
          state.receiptByPid.size === 1 &&
          state.cleanupByPid.size === 0 &&
          state.provisionalByPid.size === 0;
        requireCondition(
          !postParentReap &&
            rows.length === 2 &&
            (intentionalAcquisition || cleanupAcquisition),
          "host.ruby_topology",
          "ps introduced an unreceipted Ruby member outside atomic acquisition",
          { leg: state.leg, ordinal, rows: rows.length },
        );
        const known = [...state.receiptByPid.values()][0];
        let role;
        if (known.role === "P") {
          requireCondition(
            row.ppid === known.pid,
            "host.ruby_topology",
            "unreceipted child is not rooted at receipted parent",
            { row, known },
          );
          role = "C";
        } else {
          requireCondition(
            row.pid === state.launchPid &&
              known.ppid === row.pid,
            "host.ruby_topology",
            "unreceipted parent is not the launch leader",
            { row, known },
          );
          role = "P";
        }
        requireCondition(
          !state.roles.has(role),
          "host.ruby_topology",
          "Ruby counterpart role was already bound",
          { role, pid: row.pid },
        );
        fact = issueObservedRubyCounterpart(
          intentionalAcquisition
            ? "OneReceiptProvisionalRubyRole"
            : "CleanupBoundRubyCounterpart",
          role,
          row,
          state,
        );
        state.roles.set(role, row.pid);
        state.roleFactsByPid.set(row.pid, fact);
        (
          intentionalAcquisition
            ? state.provisionalByPid
            : state.cleanupByPid
        ).set(row.pid, fact);
      }
      const stable = stableProcessFact(row);
      const prior = state.frozen.get(row.pid);
      if (prior === undefined) {
        requireCondition(
          !isZombie(row),
          "host.ruby_identity",
          "first-seen Ruby member is a zombie",
          row,
        );
        state.frozen.set(row.pid, stable);
      } else {
        requireCondition(
          sameFact(prior, stable),
          "host.ruby_identity",
          "Ruby stable identity changed",
          { prior, current: stable },
        );
      }
      fact = state.roleFactsByPid.get(row.pid);
      requireCondition(
        fact !== undefined &&
          issuedRubyPositiveTargets.has(fact) &&
          rubyFactMatchesRow(fact, row),
        "host.ruby_identity",
        "Ruby row does not match its typed role fact",
        { leg: state.leg, ordinal, pid: row.pid },
      );
      requireCondition(
        !this.tombstones.has(row.pid),
        "host.tombstone",
        "tombstoned Ruby PID reappeared",
        row,
      );
    }

    if (postParentReap && rows.length > 0) {
      requireCondition(
        rows.length === 1 &&
          rows[0].pid === state.roles.get("C") &&
          rows[0].ppid === 1 &&
          !rows.some((row) => row.pid === state.roles.get("P")),
        "host.ruby_topology",
        "post-parent capture is not the exact frozen child/PPID-1 observation",
        { leg: state.leg, ordinal, rows },
      );
      const stable = stableProcessFact(rows[0]);
      if (state.cleanupMode === true) {
        requireCondition(
          state.roles.get("C") === rows[0].pid,
          "host.ruby_topology",
          "cleanup capture changed the exact reparented child",
          { leg: state.leg, ordinal, rows },
        );
      } else if (ordinal === 5) {
        requireCondition(
          state.reconcile05 === undefined &&
            state.promotedReparent === undefined &&
            state.confirmedReparent === undefined,
          "host.ruby_topology",
          "ordinal 05 attempted identity acquisition or reparent promotion",
        );
        state.reconcile05 = Object.freeze({
          kind: "PostParentReapReconcile05",
          childPid: rows[0].pid,
          stable,
          ppid: 1,
        });
      } else if (ordinal === 6) {
        requireCondition(
          state.reconcile05 !== undefined &&
            state.promotedReparent === undefined &&
            sameFact(state.reconcile05.stable, stable),
          "host.ruby_topology",
          "ordinal 06 cannot promote the exact reconciled child",
        );
        state.promotedReparent = Object.freeze({
          kind: "PromotedRubyReparentFact",
          childPid: rows[0].pid,
          stable,
          ppid: 1,
        });
      } else if (ordinal === 7) {
        requireCondition(
          state.promotedReparent !== undefined &&
            state.confirmedReparent === undefined &&
            sameFact(state.promotedReparent.stable, stable),
          "host.ruby_topology",
          "ordinal 07 cannot confirm the promoted child reparent fact",
        );
        state.confirmedReparent = Object.freeze({
          kind: "ConfirmedRubyReparentFact",
          childPid: rows[0].pid,
          stable,
          ppid: 1,
        });
      } else {
        requireCondition(
          ordinal >= 8 &&
            state.confirmedReparent !== undefined &&
            sameFact(state.confirmedReparent.stable, stable),
          "host.ruby_topology",
          "later survivor capture lacks the confirmed child reparent fact",
          { leg: state.leg, ordinal },
        );
      }
    }
  }

}

function heldRubyRows(state, rows) {
  const parentPid = state.roles.get("P");
  const childPid = state.roles.get("C");
  if (
    !Number.isInteger(parentPid) ||
    !Number.isInteger(childPid) ||
    rows.length !== 2
  ) {
    return false;
  }
  const parent = rows.find((row) => row.pid === parentPid);
  const child = rows.find((row) => row.pid === childPid);
  return (
    parent !== undefined &&
    child !== undefined &&
    parent.ppid === process.pid &&
    child.ppid === parent.pid &&
    parent.pgid === state.launchPid &&
    child.pgid === state.launchPid &&
    parent.sessObservedZero === "0" &&
    child.sessObservedZero === "0" &&
    isStopped(parent) &&
    isStopped(child) &&
    !isZombie(parent) &&
    !isZombie(child)
  );
}

function survivorRow(state, rows) {
  const childPid = state.roles.get("C");
  if (rows.length !== 1 || rows[0].pid !== childPid) return undefined;
  return rows[0];
}

function captureSocketIdentity(path, root, exactReceipt = undefined) {
  requireCondition(
    dirname(path) === root.path &&
      basename(path) === "control.sock" &&
      sameRoot(root.path, root),
    "host.ruby_socket",
    "Ruby socket path/root spelling differs",
    { pathHash: sha256(Buffer.from(path)) },
  );
  const first = statFact(lstatBig(path));
  const second = statFact(lstatBig(path));
  requireCondition(
    first.type === "socket" &&
      first.uid === HOST_UID &&
      first.gid === HOST_GID &&
      first.nlink === 1 &&
      sameFact(first, second) &&
      (exactReceipt === undefined ||
        (first.dev === exactReceipt.dev &&
          first.ino === exactReceipt.ino)),
    "host.ruby_socket",
    "Ruby socket identity is not stable/owned",
    { first, exactReceipt },
  );
  return Object.freeze({
    path,
    pathHash: sha256(Buffer.from(path)),
    root,
    ...first,
    source: exactReceipt === undefined ? "topology-bound" : "receipt",
  });
}

function retireSocketIdentity(identity) {
  if (absentNoFollow(identity.path)) {
    requireCondition(
      absentNoFollow(identity.path),
      "host.ruby_socket",
      "Ruby socket absence was not stable",
      { pathHash: identity.pathHash },
    );
    return Object.freeze({ alreadyAbsent: true });
  }
  requireCondition(
    sameRoot(identity.root.path, identity.root),
    "host.ruby_socket",
    "Ruby socket parent root changed",
  );
  const first = statFact(lstatBig(identity.path));
  const second = statFact(lstatBig(identity.path));
  requireCondition(
    sameFact(first, second) &&
      sameStableObject(first, identity) &&
      first.type === "socket" &&
      first.uid === HOST_UID &&
      first.nlink === 1,
    "host.ruby_socket",
    "Ruby socket is not safe to retire",
    { first, identity },
  );
  unlinkSync(identity.path);
  requireCondition(
    absentNoFollow(identity.path),
    "host.teardown",
    "Ruby socket remained after exact retirement",
    { pathHash: identity.pathHash },
  );
  return Object.freeze({ alreadyAbsent: false });
}

function retireRubyRoots(roots, socketIdentities) {
  for (const identity of socketIdentities) {
    retireSocketIdentity(identity);
  }
  for (const role of ["child", "parent", "cwd", "tmp", "home"]) {
    const receipt = roots.receipts[role];
    removeDirectory(receipt.path, receipt);
  }
}

function rubyProfileAndArgs(roots, support) {
  const profile = sandboxProfile(
    support ? "ruby-support" : "ruby-denial",
    support ? [roots.parentPath, roots.childPath] : [],
  );
  const args = [
    "-p",
    profile.bytes.toString("utf8"),
    RUBY,
    "--disable=gems,rubyopt,did_you_mean",
    `-I${RUBY_PLATFORM}`,
    `-I${RUBY_BASE}`,
    "-rsocket",
    "-e",
    RUBY_LITERAL,
    "--",
    roots.parentPath,
    roots.childPath,
  ];
  return Object.freeze({ profile, args: Object.freeze(args) });
}

const RUBY_BRANCH_ORDINALS = Object.freeze({
  ANCHORED: Object.freeze([12, 13, 14]),
  SHORT: Object.freeze([1, 2, 3, 12, 13, 14]),
  CHILD_FIRST: Object.freeze([
    1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,
  ]),
  PARENT_FIRST: Object.freeze([
    1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,
  ]),
});

const RUBY_CLEANUP_SUFFIX_TABLE = Object.freeze(
  Object.entries(RUBY_BRANCH_ORDINALS).flatMap(
    ([branchKey, branch]) =>
      [0, ...branch].map((lastOrdinal) =>
        Object.freeze({
          branchKey,
          lastOrdinal,
          remaining: Object.freeze(
            branch.filter((ordinal) => ordinal > lastOrdinal),
          ),
        })
      ),
  ),
);

function rubyBranchKey(leg, firstRole = undefined) {
  if (leg === "support" || leg === "denial") return "SHORT";
  if (leg === "one-receipt" && firstRole === "C") {
    return "CHILD_FIRST";
  }
  return "PARENT_FIRST";
}

function rubyBranch(leg, firstRole = undefined) {
  return RUBY_BRANCH_ORDINALS[rubyBranchKey(leg, firstRole)];
}

function rubyCleanupSuffix(branchKey, lastOrdinal) {
  const matches = RUBY_CLEANUP_SUFFIX_TABLE.filter(
    (entry) =>
      entry.branchKey === branchKey &&
      entry.lastOrdinal === lastOrdinal,
  );
  requireCondition(
    matches.length === 1,
    "host.ruby_cleanup_suffix",
    "Ruby cleanup suffix lookup is not closed and unique",
    { branchKey, lastOrdinal, matches: matches.length },
  );
  return matches[0].remaining;
}

async function requireNormalRubyTerminalAndOutput(
  state,
  stdout,
  stderr,
  expected,
) {
  const [terminal, stdoutResult, stderrResult] = await Promise.all([
    waitFor(
      state.deadline.sub("direct-reap", 2_000),
      state.launched.terminal.promise,
      "host.ruby_terminal",
      `${state.leg} direct Ruby parent did not reap`,
    ),
    stdout.promise,
    stderr.promise,
  ]);
  requireExactTerminal(
    terminal,
    state.leg === "denial" ? "host.ruby_denial" : "host.ruby_support",
    state.leg,
  );
  state.directTerminal = terminal;
  state.parentReaped = true;
  requireCondition(
    stdoutResult.eof &&
      stderrResult.eof &&
      stdoutResult.body.equals(Buffer.from(`${expected}\n`)) &&
      stderrResult.bytes === 0,
    "host.ruby_output",
    `${state.leg} stdout/stderr differs`,
    {
      terminal,
      stdout: captureFacts(stdoutResult),
      stderr: captureFacts(stderrResult),
    },
  );
  return Object.freeze({ terminal, stdoutResult, stderrResult });
}

async function captureTerminalAbsence(state, proofOwner) {
  const first = await proofOwner.capture(
    state,
    13,
    (rows) => rows.length === 0,
    "terminal_absence_1",
  );
  await delay(state.deadline, 5);
  const second = await proofOwner.capture(
    state,
    14,
    (rows) => rows.length === 0,
    "terminal_absence_2",
  );
  return Object.freeze({ first, second });
}

function rubyRoleSignalTarget(state, role) {
  const pid = state.roles.get(role);
  const fact = state.roleFactsByPid.get(pid);
  requireCondition(
    (role === "P" || role === "C") &&
      Number.isSafeInteger(pid) &&
      fact !== undefined &&
      issuedRubyPositiveTargets.has(fact),
    "host.signal_authority",
    "Ruby role has no typed positive signal target",
    { role },
  );
  return fact;
}

function rubyCaptureTransition(captureAction, state, proof) {
  requireCondition(
    RUBY_CAPTURE_ACTIONS.includes(captureAction),
    "host.signal_authority",
    "Ruby capture action is outside the closed domain",
    { captureAction },
  );
  if (captureAction === "NONE") {
    return Object.freeze({
      signal: null,
      targetFact: null,
      consume: false,
      alreadyStopped: false,
    });
  }
  if (
    captureAction === "GROUP_CONT" ||
    captureAction === "GROUP_KILL" ||
    captureAction === "GROUP_KILL_IF_PRESENT"
  ) {
    const signal =
      captureAction === "GROUP_CONT" ? "SIGCONT" : "SIGKILL";
    const present =
      captureAction !== "GROUP_KILL_IF_PRESENT" ||
      proof.rows.length > 0;
    return Object.freeze({
      signal: present ? signal : null,
      targetFact: present ? state.scope : null,
      consume: true,
      alreadyStopped: false,
    });
  }
  if (captureAction === "PARENT_KILL") {
    return Object.freeze({
      signal: "SIGKILL",
      targetFact: rubyRoleSignalTarget(state, "P"),
      consume: true,
      alreadyStopped: false,
    });
  }
  const child = proof.rows.length === 1 ? proof.rows[0] : null;
  if (captureAction === "SURVIVOR_STOP") {
    requireCondition(
      child !== null && child.pid === state.roles.get("C"),
      "host.signal_authority",
      "Ruby survivor STOP capture lacks the exact child",
    );
    return Object.freeze({
      signal: isStopped(child) ? null : "SIGSTOP",
      targetFact: isStopped(child)
        ? null
        : rubyRoleSignalTarget(state, "C"),
      consume: true,
      alreadyStopped: isStopped(child),
    });
  }
  if (captureAction === "SURVIVOR_CONT") {
    requireCondition(
      child !== null &&
        child.pid === state.roles.get("C") &&
        isStopped(child),
      "host.signal_authority",
      "Ruby survivor CONT capture lacks the stopped child",
    );
    return Object.freeze({
      signal: "SIGCONT",
      targetFact: rubyRoleSignalTarget(state, "C"),
      consume: true,
      alreadyStopped: false,
    });
  }
  if (captureAction === "SURVIVOR_TERM") {
    requireCondition(
      child === null || child.pid === state.roles.get("C"),
      "host.signal_authority",
      "Ruby survivor TERM capture has an unexpected row",
    );
    return Object.freeze({
      signal: child === null ? null : "SIGTERM",
      targetFact:
        child === null ? null : rubyRoleSignalTarget(state, "C"),
      consume: child !== null,
      alreadyStopped: false,
    });
  }
  requireCondition(
    captureAction === "SURVIVOR_KILL" &&
      (child === null || child.pid === state.roles.get("C")),
    "host.signal_authority",
    "Ruby survivor KILL capture has an unexpected row/action",
  );
  const shouldKill = child !== null && !isZombie(child);
  return Object.freeze({
    signal: shouldKill ? "SIGKILL" : null,
    targetFact:
      shouldKill ? rubyRoleSignalTarget(state, "C") : null,
    consume: true,
    alreadyStopped: false,
  });
}

async function runOrphanRoute(state, proofOwner, parentKilled) {
  if (parentKilled) {
    const directTerminal = await waitFor(
      state.deadline.sub("direct-parent-reap", 2_000),
      state.launched.terminal.promise,
      "host.ruby_terminal",
      "killed Ruby direct parent did not reap",
    );
    requireCondition(
      directTerminal.error === null &&
        directTerminal.code === null &&
        directTerminal.signal === "SIGKILL",
      "host.ruby_terminal",
      "Ruby direct-parent KILL terminal differs",
      directTerminal,
    );
    state.directTerminal = directTerminal;
    state.parentReaped = true;
  }
  const reconcile = await proofOwner.capture(
    state,
    5,
    (rows) =>
      rows.length === 0 ||
      (survivorRow(state, rows) !== undefined &&
        !rows.some((row) => row.pid === state.roles.get("P"))),
    "direct_parent_reap_reconcile",
  );
  let rows = reconcile.rows;
  if (rows.length === 0) {
    return;
  }
  await proofOwner.capture(
    state,
    6,
    (current) => {
      const child = survivorRow(state, current);
      return child !== undefined && child.ppid === 1;
    },
    "survivor_reparent_acquire",
  );
  await proofOwner.capture(
    state,
    7,
    (current) => {
      const child = survivorRow(state, current);
      return child !== undefined && child.ppid === 1;
    },
    "survivor_reparent_confirm",
  );
  await proofOwner.capture(
    state,
    8,
    (current) => {
      const child = survivorRow(state, current);
      return (
        child !== undefined &&
        child.ppid === 1 &&
        !isZombie(child)
      );
    },
    "survivor_pre_stop",
    "SURVIVOR_STOP",
  );
  await proofOwner.capture(
    state,
    9,
    (current) => {
      const child = survivorRow(state, current);
      return child !== undefined && child.ppid === 1 && isStopped(child);
    },
    "survivor_stopped_confirm",
  );
  await proofOwner.capture(
    state,
    10,
    (current) => {
      const child = survivorRow(state, current);
      return child !== undefined && child.ppid === 1 && isStopped(child);
    },
    "survivor_pre_cont",
    "SURVIVOR_CONT",
  );
  const preTerm = await proofOwner.capture(
    state,
    11,
    (current) =>
      current.length === 0 ||
      (() => {
        const child = survivorRow(state, current);
        return (
          child !== undefined &&
          child.ppid === 1 &&
          !isZombie(child)
        );
      })(),
    "live_pre_term",
    "SURVIVOR_TERM",
  );
  if (preTerm.rows.length === 0) return;
  await delay(state.deadline, 25);
  const preKill = await proofOwner.capture(
    state,
    12,
    (current) =>
      current.length === 0 ||
      (() => {
        const child = survivorRow(state, current);
        return (
          child !== undefined &&
          child.ppid === 1 &&
          (isZombie(child) || !isStopped(child))
        );
      })(),
    "live_pre_kill",
    "SURVIVOR_KILL",
  );
  if (preKill.rows.length === 1 && !isZombie(preKill.rows[0])) {
    await delay(state.deadline, 10);
  }
}

function tombstoneRubyMembers(state, tombstones) {
  for (const [role, pid] of [...state.roles.entries()].sort()) {
    if (!tombstones.has(pid)) {
      tombstones.add(pid, `RUBY_${role}_TERMINAL_ABSENCE`);
    }
  }
}

function cleanupRubyRows(state, rows) {
  return (
    rows.length <= 2 &&
    rows.every(
      (row) =>
        row.pgid === state.launchPid &&
        row.sessObservedZero === "0" &&
        row.uid === HOST_UID &&
        row.ucomm === "ruby" &&
        [...state.roles.values()].includes(row.pid),
    )
  );
}

class RubyBranchCleanupOwner {
  constructor({
    state,
    support,
    proofOwner,
    receiptReader,
    receiptTransport,
    stdout,
    stderr,
    fifo,
    batch,
    evidence,
    tombstones,
  }) {
    this.state = state;
    this.support = support;
    this.proofOwner = proofOwner;
    this.receiptReader = receiptReader;
    this.receiptTransport = receiptTransport;
    this.stdout = stdout;
    this.stderr = stderr;
    this.fifo = fifo;
    this.batch = batch;
    this.evidence = evidence;
    this.tombstones = tombstones;
    this.socketIdentities = [];
    this.receiptClosed = false;
    this.rootsRetired = false;
    this.fifoRetired = false;
    this.running = false;
  }

  setSocketIdentities(identities) {
    requireCondition(
      this.socketIdentities.length === 0 &&
        Array.isArray(identities) &&
        identities.length <= (this.support ? 2 : 0),
      "host.ruby_cleanup_state",
      "Ruby cleanup socket identity handoff differs",
      { support: this.support, identities: identities.length },
    );
    this.socketIdentities = [...identities];
  }

  closeReceiptReader() {
    if (!this.receiptClosed) {
      this.receiptReader.close();
      this.receiptClosed = true;
    }
  }

  collectSocketIdentities() {
    if (!this.support) {
      requireCondition(
        absentNoFollow(this.state.roots.parentPath) &&
          absentNoFollow(this.state.roots.childPath),
        "host.ruby_cleanup_state",
        "Ruby denial cleanup observed a socket pathname",
      );
      this.setSocketIdentities([]);
      return;
    }
    const identities = [];
    for (const [role, path, root] of [
      [
        "P",
        this.state.roots.parentPath,
        this.state.roots.receipts.parent,
      ],
      [
        "C",
        this.state.roots.childPath,
        this.state.roots.receipts.child,
      ],
    ]) {
      if (absentNoFollow(path)) {
        requireCondition(
          absentNoFollow(path),
          "host.ruby_cleanup_state",
          "Ruby cleanup socket absence was not stable",
          { role },
        );
      } else {
        identities.push(
          captureSocketIdentity(
            path,
            root,
            this.receiptTransport.records.find(
              (record) => record.role === role,
            ),
          ),
        );
      }
    }
    this.setSocketIdentities(identities);
  }

  retireRoots() {
    if (!this.rootsRetired) {
      retireRubyRoots(this.state.roots, this.socketIdentities);
      this.rootsRetired = true;
    }
  }

  retireFifo(intentionallyClosedReceipt) {
    if (!this.fifoRetired) {
      this.fifo.retire(
        this.batch,
        this.state.deadline,
        intentionallyClosedReceipt ? new Set([0]) : new Set(),
      );
      this.fifoRetired = true;
      this.receiptClosed = true;
    }
  }

  async run(operation) {
    requireCondition(
      !this.running,
      "host.ruby_cleanup_state",
      "Ruby branch cleanup owner was entered more than once",
      { leg: this.state.leg },
    );
    this.running = true;
    try {
      return await operation();
    } catch (error) {
      const firstFault =
        error instanceof HostAuthorityError
          ? error
          : new HostAuthorityError(
              "host.ruby_branch_failure",
              "Ruby branch raised a non-typed first fault",
              safeError(error),
            );
      let cleanupError;
      try {
        await this.cleanup(firstFault);
      } catch (secondary) {
        cleanupError = secondary;
      }
      throw aggregate(firstFault, [cleanupError]);
    }
  }

  async cleanup(firstFault) {
    const state = this.state;
    state.cleanupMode = true;
    const initialLastOrdinal = state.lastOrdinal;
    const remaining =
      state.scope === undefined
        ? Object.freeze([])
        : rubyCleanupSuffix(
            state.branchKey,
            initialLastOrdinal,
          );
    this.closeReceiptReader();

    if (state.scope === undefined) {
      this.evidence.add(
        "ruby",
        "ruby.unscoped_failure",
        {
          leg: state.leg,
          firstFault: safeError(firstFault),
          receipt: captureFacts(this.receiptTransport),
          possibleUnreceiptedFork: true,
          groupCaptures: 0,
          groupSignals: 0,
          rootsRetained: true,
        },
        state.deadline,
      );
      return;
    }

    let custody = null;
    const executed = [];
    const deferredCaptureFaults = [];
    for (const ordinal of remaining.filter(
      (candidate) => candidate <= 12,
    )) {
      if (
        state.branchKey === "ANCHORED" &&
        ordinal === 12
      ) {
        const captured =
          await this.proofOwner.captureReceiptFaultCustody(
            state,
            (rows) => cleanupRubyRows(state, rows),
            "receipt_early_cleanup_pre_kill",
          );
        custody = captured.custody;
        if (captured.deferredFault !== undefined) {
          deferredCaptureFaults.push(
            captured.deferredFault,
          );
        }
      } else {
        try {
          await this.proofOwner.capture(
            state,
            ordinal,
            (rows) => cleanupRubyRows(state, rows),
            `branch_failure_suffix_${String(ordinal).padStart(2, "0")}`,
            ordinal === 12
              ? "GROUP_KILL_IF_PRESENT"
              : "NONE",
          );
        } catch (error) {
          requireCondition(
            state.lastOrdinal === ordinal &&
              state.proofs.has(ordinal),
            "host.ruby_cleanup_suffix",
            "Ruby cleanup capture failed before consuming its exact ordinal",
            {
              leg: state.leg,
              ordinal,
              lastOrdinal: state.lastOrdinal,
              captureFault: safeError(error),
            },
          );
          deferredCaptureFaults.push(error);
        }
      }
      executed.push(ordinal);
    }

    if (!state.parentReaped) {
      const terminal = await waitFor(
        state.deadline,
        state.launched.terminal.promise,
        "host.ruby_cleanup_terminal",
        "Ruby branch cleanup did not reap its direct parent",
      );
      requireCondition(
        terminal.error === null &&
          ((terminal.signal === "SIGKILL" &&
            terminal.code === null) ||
            (terminal.signal === null &&
              terminal.code === 0)),
        "host.ruby_cleanup_terminal",
        "Ruby branch cleanup direct terminal differs",
        terminal,
      );
      state.directTerminal = terminal;
      state.parentReaped = true;
    }

    for (const ordinal of remaining.filter(
      (candidate) => candidate >= 13,
    )) {
      const priorRows = state.proofRows.get(ordinal);
      if (priorRows !== undefined) {
        requireCondition(
          priorRows.length === 0,
          "host.ruby_cleanup_suffix",
          "prior terminal ordinal was not an absence proof",
          { leg: state.leg, ordinal, rows: priorRows.length },
        );
      } else {
        requireCondition(
          state.branch.includes(ordinal) &&
            ordinal > state.lastOrdinal,
          "host.ruby_cleanup_suffix",
          "Ruby cleanup cannot complete both terminal absences",
          {
            leg: state.leg,
            ordinal,
            lastOrdinal: state.lastOrdinal,
          },
        );
        try {
          await this.proofOwner.capture(
            state,
            ordinal,
            (rows) => rows.length === 0,
            `branch_failure_absence_${ordinal}`,
          );
        } catch (error) {
          requireCondition(
            state.lastOrdinal === ordinal &&
              state.proofs.has(ordinal),
            "host.ruby_cleanup_suffix",
            "Ruby cleanup absence capture failed before consuming its ordinal",
            {
              leg: state.leg,
              ordinal,
              lastOrdinal: state.lastOrdinal,
              captureFault: safeError(error),
            },
          );
          deferredCaptureFaults.push(error);
        }
      }
      executed.push(ordinal);
    }

    const [stdoutResult, stderrResult] = await Promise.all([
      this.stdout.promise,
      this.stderr.promise,
    ]);
    requireCondition(
      stdoutResult.eof && stderrResult.eof,
      "host.ruby_cleanup_output",
      "Ruby branch cleanup did not reach output EOF",
      {
        stdout: captureFacts(stdoutResult),
        stderr: captureFacts(stderrResult),
      },
    );
    tombstoneRubyMembers(state, this.tombstones);
    if (!this.tombstones.has(state.launched.pid)) {
      this.tombstones.add(state.launched.pid, "RUBY_DIRECT_REAP");
    }
    if (this.socketIdentities.length === 0) {
      this.collectSocketIdentities();
    }
    this.retireRoots();
    this.retireFifo(true);
    this.evidence.add(
      "ruby",
      state.branchKey === "ANCHORED"
        ? "ruby.receipt_fault_cleanup"
        : "ruby.branch_failure_cleanup",
      {
        leg: state.leg,
        branchKey: state.branchKey,
        firstFault: safeError(firstFault),
        initialLastOrdinal,
        remaining,
        executed,
        custodySha256:
          custody === null
            ? null
            : sha256(Buffer.from(canonicalJson(custody))),
        terminal: state.directTerminal,
        stdout: captureFacts(stdoutResult),
        stderr: captureFacts(stderrResult),
        consumedOrdinals: [...state.consumed].sort(
          (left, right) => left - right,
        ),
      },
      state.deadline,
    );
    if (deferredCaptureFaults.length > 0) {
      throw aggregate(undefined, deferredCaptureFaults);
    }
  }
}

async function runRubyLeg(
  worker,
  fifo,
  proofOwner,
  evidence,
  tombstones,
  legIndex,
  leg,
  containerReceipt,
  previousRoots,
) {
  const deadline = worker.deadline.sub(
    `ruby-${leg}`,
    DEADLINE_MS.ruby,
  );
  const support = leg !== "denial";
  const reservations = reserveRubyLeg(worker.capacity, support);
  const roots = createRubyRoots(
    worker,
    containerReceipt,
    previousRoots,
  );
  const batch = await fifo.create(
    36 + legIndex,
    ["receipt.fifo", "stdout.fifo", "stderr.fifo"],
    deadline.sub("fifo", DEADLINE_MS.fifoBatch),
  );
  const intentionalOneReceipt = leg === "one-receipt";
  const ordinaryOperationLeg =
    leg === "support"
      ? "SUPPORT"
      : leg === "denial"
        ? "DENIAL"
        : leg === "parent-loss"
          ? "PARENT_LOSS"
          : null;
  const receiptReader = rubyReceiptReader(
    batch.endpoints[0].reader,
    deadline,
    intentionalOneReceipt ? "ONE_RECEIPT" : "ORDINARY",
    ordinaryOperationLeg,
  );
  const stdout = channelReader(
    batch.endpoints[1].reader,
    "Ruby stdout",
    64,
    deadline,
  );
  const stderr = channelReader(
    batch.endpoints[2].reader,
    "Ruby stderr",
    4_096,
    deadline,
  );
  const launch = rubyProfileAndArgs(roots, support);
  const stdin = openDevNull(fsConstants.O_RDONLY, deadline);
  let launched;
  try {
    requireProtocolPeak(15);
    launched = spawnExact({
      executable: SANDBOX_EXEC,
      args: launch.args,
      cwd: roots.receipts.cwd.path,
      env: closedEnvironment(
        roots.receipts.home.path,
        roots.receipts.tmp.path,
      ),
      stdio: [
        stdin.fd,
        batch.endpoints[1].writer,
        batch.endpoints[2].writer,
        batch.endpoints[0].writer,
      ],
      detached: true,
      label: `ruby-${leg}`,
      tombstones,
    });
  } finally {
    checkedClose(stdin.fd, deadline);
  }
  closeHandoff(batch);
  const receiptTransport = await receiptReader.promise;
  if (
    receiptTransport.latch.outcome === "TYPED_STOP" &&
    receiptTransport.latch.proofRoute ===
      "RECEIPT_NO_ANCHOR_DIRECT_ONLY"
  ) {
    fail(
      "host.ruby_receipt_terminal",
      "Ruby receipt transport stopped without group authority",
      {
        latch: receiptTransport.latch,
        counters: receiptTransport.counters,
        positiveDirectLaunchPid: launched.pid,
        possibleUnreceiptedFork: true,
        readerCloseClaim: false,
        groupCaptures: 0,
        groupSignals: 0,
        rootsRetained: true,
      },
    );
  }
  const receiptRecords = receiptTransport.records;
  requireCondition(
    receiptRecords.length >= 1 &&
      receiptRecords.length <= 2,
    "host.ruby_receipt_terminal",
    "Ruby receipt terminal did not project a bounded authority prefix",
    { latch: receiptTransport.latch },
  );
  const firstRole = receiptRecords[0].role;
  const anchoredReceiptFault =
    receiptTransport.latch.outcome === "TYPED_STOP" &&
    receiptTransport.latch.proofRoute ===
      "RECEIPT_ANCHORED_EARLY_CLEANUP";
  const branchKey = rubyBranchKey(leg, firstRole);
  const state = {
    leg,
    launchPid: launched.pid,
    launched,
    deadline,
    roots,
    intentionalOneReceipt,
    firstRole,
    branchKey: anchoredReceiptFault
      ? "ANCHORED"
      : branchKey,
    branch:
      RUBY_BRANCH_ORDINALS[
        anchoredReceiptFault
          ? "ANCHORED"
          : branchKey
      ],
    consumed: new Set(),
    lastOrdinal: 0,
    proofRows: new Map(),
    proofs: new Map(),
    actions: new Map(),
    cleanupMode: false,
    receiptProofRoute: receiptTransport.latch.proofRoute,
    receiptClassifiedRoute:
      receiptTransport.latch.classifiedRoute,
    receiptByPid: new Map(),
    roles: new Map(),
    roleFactsByPid: new Map(),
    provisionalByPid: new Map(),
    cleanupByPid: new Map(),
    frozen: new Map(),
    directTerminal: undefined,
    parentReaped: false,
    reconcile05: undefined,
    promotedReparent: undefined,
    confirmedReparent: undefined,
    scope: undefined,
  };
  const cleanupOwner = new RubyBranchCleanupOwner({
    state,
    support,
    proofOwner,
    receiptReader,
    receiptTransport,
    stdout,
    stderr,
    fifo,
    batch,
    evidence,
    tombstones,
  });
  return cleanupOwner.run(async () => {
    for (const record of receiptRecords) {
    requireCondition(
      record.branch === (support ? "S" : "D") &&
        record.pgid === launched.pid &&
        record.sid === launched.pid &&
        ((record.role === "P" &&
          record.pid === launched.pid &&
          record.ppid === process.pid) ||
          (record.role === "C" &&
            record.pid !== launched.pid &&
            record.ppid === launched.pid)),
      "host.ruby_topology",
      "Ruby receipt does not establish its launch-rooted role",
      { record, launchedPid: launched.pid, workerPid: process.pid },
    );
    state.receiptByPid.set(record.pid, record);
    state.roles.set(record.role, record.pid);
    }
    state.scope = issueRubyScope(launched.pid, receiptRecords);
    if (anchoredReceiptFault) {
    fail(
      "host.ruby_receipt_terminal",
      "Ruby receipt transport selected bounded anchored cleanup",
      {
        latch: receiptTransport.latch,
        counters: receiptTransport.counters,
      },
    );
    }
    if (receiptRecords.length === 2) {
      validateRubyReceiptTopology(
      state,
      receiptRecords,
      support ? "S" : "D",
    );
    }
  requireCondition(
    receiptTransport.latch.outcome ===
      (intentionalOneReceipt
        ? "ONE_RECEIPT_ELIGIBLE_AFTER_CLEANUP"
        : "ORDINARY_SUCCESS") &&
      receiptTransport.latch.selectedLeg ===
        (intentionalOneReceipt
          ? firstRole === "P"
            ? "ONE_RECEIPT_P_FIRST"
            : "ONE_RECEIPT_C_FIRST"
          : ordinaryOperationLeg),
    "host.ruby_receipt_terminal",
    "Ruby receipt transport selected a non-eligible terminal",
    {
      latch: receiptTransport.latch,
      counters: receiptTransport.counters,
    },
  );
  const scope = await proofOwner.capture(
    state,
    1,
    (rows) => heldRubyRows(state, rows),
    "scope_acquire",
  );
  await proofOwner.capture(
    state,
    2,
    (rows) => heldRubyRows(state, rows),
    "scope_confirm",
  );
  if (intentionalOneReceipt) {
    cleanupOwner.closeReceiptReader();
  }
  let socketIdentities = [];
  if (support) {
    const parentReceipt = receiptRecords.find(
      (record) => record.role === "P",
    );
    const childReceipt = receiptRecords.find(
      (record) => record.role === "C",
    );
    socketIdentities = [
      captureSocketIdentity(
        roots.parentPath,
        roots.receipts.parent,
        parentReceipt,
      ),
      captureSocketIdentity(
        roots.childPath,
        roots.receipts.child,
        childReceipt,
      ),
    ];
  } else {
    requireCondition(
      absentNoFollow(roots.parentPath) &&
        absentNoFollow(roots.childPath),
      "host.ruby_denial",
      "Ruby denial paths appeared after stable hold",
    );
  }
  cleanupOwner.setSocketIdentities(socketIdentities);
  let terminalFacts;
  if (leg === "support" || leg === "denial") {
    await proofOwner.capture(
      state,
      3,
      (rows) => heldRubyRows(state, rows),
      "pre_cont",
      "GROUP_CONT",
    );
    terminalFacts = await requireNormalRubyTerminalAndOutput(
      state,
      stdout,
      stderr,
      leg === "denial" ? "DENIED" : "SUCCESS",
    );
    state.parentReaped = true;
  } else if (leg === "one-receipt" && firstRole === "C") {
    await proofOwner.capture(
      state,
      3,
      (rows) => heldRubyRows(state, rows),
      "pre_cont",
      "GROUP_CONT",
    );
    terminalFacts = await requireNormalRubyTerminalAndOutput(
      state,
      stdout,
      stderr,
      "SUCCESS",
    );
    state.parentReaped = true;
    await runOrphanRoute(state, proofOwner, false);
  } else {
    await proofOwner.capture(
      state,
      4,
      (rows) =>
        heldRubyRows(state, rows) &&
        rows.some(
          (row) =>
            row.pid === state.launchPid &&
            isStopped(row) &&
            !isZombie(row),
      ),
      "direct_parent_pre_kill",
      "PARENT_KILL",
    );
    await runOrphanRoute(state, proofOwner, true);
    const [stdoutResult, stderrResult] = await Promise.all([
      stdout.promise,
      stderr.promise,
    ]);
    requireCondition(
      stdoutResult.eof &&
        stderrResult.eof &&
        stdoutResult.bytes === 0 &&
        stderrResult.bytes === 0,
      "host.ruby_fault_output",
      `${leg} fault output differs`,
      {
        stdout: captureFacts(stdoutResult),
        stderr: captureFacts(stderrResult),
      },
    );
    terminalFacts = Object.freeze({
      terminal: state.directTerminal,
      stdoutResult,
      stderrResult,
    });
  }
  await captureTerminalAbsence(state, proofOwner);
  tombstoneRubyMembers(state, tombstones);
  if (leg === "support" || leg === "denial" ||
      (leg === "one-receipt" && firstRole === "C")) {
    if (!tombstones.has(launched.pid)) {
      tombstones.add(launched.pid, "RUBY_DIRECT_REAP");
    }
  }
  cleanupOwner.retireRoots();
  cleanupOwner.retireFifo(intentionalOneReceipt);
  evidence.add(
    "ruby",
    `ruby.${leg}`,
    {
      legIndex,
      literalSha256: sha256(Buffer.from(RUBY_LITERAL)),
      profileSha256: launch.profile.sha256,
      argvSha256: sha256(
        Buffer.from(
          canonicalJson([
            "-p",
            launch.profile.sha256,
            RUBY,
            "--disable=gems,rubyopt,did_you_mean",
            `-I${RUBY_PLATFORM}`,
            `-I${RUBY_BASE}`,
            "-rsocket",
            "-e",
            sha256(Buffer.from(RUBY_LITERAL)),
            "--",
            sha256(Buffer.from(roots.parentPath)),
            sha256(Buffer.from(roots.childPath)),
          ]),
        ),
      ),
      environmentSha256: sha256(
        Buffer.from(
          canonicalJson(
            closedEnvironment(
              roots.receipts.home.path,
              roots.receipts.tmp.path,
            ),
          ),
        ),
      ),
      receipt: captureFacts(receiptTransport),
      receiptRaw: s2RawFact(receiptTransport.body),
      receiptLatch: receiptTransport.latch,
      receiptCounters: receiptTransport.counters,
      firstRole,
      receiptRecords,
      terminal: terminalFacts.terminal,
      stdout: captureFacts(terminalFacts.stdoutResult),
      stderr: captureFacts(terminalFacts.stderrResult),
      consumedOrdinals: [...state.consumed].sort((left, right) => left - right),
      scopeSha256: sha256(Buffer.from(canonicalJson(scope))),
      socketSources: socketIdentities.map((identity) => identity.source),
    },
    deadline,
  );
  worker.capacity.complete(reservations.leg);
  if (reservations.sockets !== null) {
    worker.capacity.complete(reservations.sockets);
  }
  return Object.freeze({
    rootReceipts: Object.freeze(
      Object.fromEntries(
        Object.entries(roots.receipts).map(([role, receipt]) => [
          role,
          Object.freeze({
            pathHash: receipt.pathHash,
            dev: receipt.dev,
            ino: receipt.ino,
          }),
        ]),
      ),
    ),
    firstRole,
    consumedOrdinals: Object.freeze(
      [...state.consumed].sort((left, right) => left - right),
    ),
  });
  });
}

class S2CaptureReservationOwner {
  constructor(capacity) {
    requireCondition(
      capacity instanceof CapacityLedger,
      "host.capture_capacity",
      "S2 capture reservation owner lacks its capacity ledger",
    );
    this.capacity = capacity;
    this.nextBatch = 0;
    this.attemptCount = 0;
    this.replacementUsed = false;
  }

  reserveOrdinary() {
    const batchIndex = this.nextBatch;
    requireCondition(
      batchIndex < 36,
      "host.proof_capacity",
      "S2 ordinary capture batch exceeded 36",
      { batchIndex },
    );
    const reservations = reserveS2CaptureAttempt(
      this.capacity,
    );
    this.nextBatch += 1;
    return Object.freeze({
      batchIndex,
      replacement: false,
      reservations,
    });
  }

  reserveReplacement() {
    requireCondition(
      !this.replacementUsed &&
        this.attemptCount >= 1 &&
        this.attemptCount <= 36,
      "CLEANUP_OBSERVATION_BUDGET_EXHAUSTED",
      "S2 cleanup replacement is unavailable",
      {
        replacementUsed: this.replacementUsed,
        attemptCount: this.attemptCount,
      },
    );
    const reservations = reserveS2CaptureAttempt(
      this.capacity,
    );
    this.replacementUsed = true;
    return Object.freeze({
      batchIndex: 42,
      replacement: true,
      reservations,
    });
  }

  begin(admission) {
    requireCondition(
      admission !== null &&
        typeof admission === "object" &&
        admission.reservations !== null &&
        typeof admission.reservations === "object" &&
        admission.reservations.attempt !== undefined &&
        admission.reservations.ps !== undefined &&
        this.attemptCount < LIMITS.captureAttempts &&
        (admission.replacement
          ? admission.batchIndex === 42 &&
            this.replacementUsed
          : admission.batchIndex >= 0 &&
            admission.batchIndex < 36),
      "CLEANUP_OBSERVATION_BUDGET_EXHAUSTED",
      "S2 capture admission differs",
      {
        admission,
        attempts: this.attemptCount,
      },
    );
    this.attemptCount += 1;
    return this.attemptCount;
  }

  snapshot() {
    return Object.freeze({
      nextBatch: this.nextBatch,
      attemptCount: this.attemptCount,
      replacementUsed: this.replacementUsed,
      capacity: this.capacity.snapshot(),
    });
  }
}

const S2_CAPTURE_TRANSITIONS = Object.freeze({
  START: Object.freeze(["Reserved"]),
  Reserved: Object.freeze([
    "AttemptMaterialized",
    "RetiredNoObject",
  ]),
  AttemptMaterialized: Object.freeze([
    "CaptureLaunched",
    "Retired",
    "RetirementFault",
  ]),
  CaptureLaunched: Object.freeze([
    "ProofInstalled",
    "Retired",
    "RetirementFault",
  ]),
  ProofInstalled: Object.freeze([
    "Retired",
    "RetirementFault",
  ]),
  Retired: Object.freeze([
    "EvidenceCommitted",
    "EvidenceCommitFault",
  ]),
  RetiredNoObject: Object.freeze([
    "EvidenceCommitted",
    "EvidenceCommitFault",
  ]),
  RetirementFault: Object.freeze([
    "Retired",
    "RetirementFault",
  ]),
  EvidenceCommitted: Object.freeze([]),
  EvidenceCommitFault: Object.freeze([]),
});

class S2CaptureLifecycle {
  constructor() {
    this.states = [];
  }

  enter(state) {
    const previous =
      this.states.length === 0
        ? "START"
        : this.states.at(-1);
    requireCondition(
      Object.hasOwn(S2_CAPTURE_TRANSITIONS, previous) &&
        S2_CAPTURE_TRANSITIONS[previous].includes(state),
      "host.capture_attempt",
      "S2 capture lifecycle transition differs",
      { previous, state, states: this.states },
    );
    this.states.push(state);
  }

  snapshot() {
    return Object.freeze([...this.states]);
  }

  includes(state) {
    return this.states.includes(state);
  }
}

function s2CaptureTypedFault(
  firstFault,
  secondaryFaults,
  lifecycle,
  replacementEligible,
) {
  return new HostAuthorityError(
    firstFault.code,
    firstFault.message,
    {
      firstFault: safeError(firstFault),
      secondaryFaults: secondaryFaults.map(safeError),
      states: lifecycle.snapshot(),
      replacementEligible,
      retained:
        firstFault.data?.retained === true ||
        secondaryFaults.length > 0 ||
        lifecycle.includes("EvidenceCommitFault") ||
        lifecycle.includes("RetirementFault"),
    },
  );
}

class S2CaptureAttemptOwner {
  constructor(worker, fifo, evidence, tombstones, canary) {
    this.worker = worker;
    this.fifo = fifo;
    this.evidence = evidence;
    this.tombstones = tombstones;
    this.canary = canary;
    this.reservationOwner =
      new S2CaptureReservationOwner(worker.capacity);
  }

  async capture(
    state,
    ordinal,
    predicate,
    label,
    deferCommit = false,
  ) {
    requireCondition(
      S2_PROOF_ORDINALS[state.leg].includes(ordinal) &&
        !state.consumed.has(ordinal) &&
        ordinal > state.lastOrdinal,
      "host.proof_ordinal",
      "S2 proof ordinal is unavailable",
      {
        leg: state.leg,
        ordinal,
        consumed: [...state.consumed],
      },
    );
    const ordinaryAdmission =
      this.reservationOwner.reserveOrdinary();
    try {
      return await this.#captureAttempt(
        state,
        ordinal,
        predicate,
        label,
        deferCommit,
        ordinaryAdmission,
      );
    } catch (error) {
      if (
        error instanceof HostAuthorityError &&
        error.data?.replacementEligible === true
      ) {
        let replacementAdmission;
        try {
          replacementAdmission =
            this.reservationOwner.reserveReplacement();
        } catch (reservationError) {
          throw new HostAuthorityError(
            error.code,
            error.message,
            {
              firstFault: safeError(error),
              secondaryFaults: [safeError(reservationError)],
              replacementEligible: false,
              retained: true,
            },
          );
        }
        try {
          return await this.#captureAttempt(
            state,
            ordinal,
            predicate,
            `${label}-replacement`,
            deferCommit,
            replacementAdmission,
          );
        } catch (replacementError) {
          throw new HostAuthorityError(
            error.code,
            error.message,
            {
              firstFault: safeError(error),
              secondaryFaults: [safeError(replacementError)],
              replacementEligible: false,
              retained: true,
            },
          );
        }
      }
      throw error;
    }
  }

  async #captureAttempt(
    state,
    ordinal,
    predicate,
    label,
    deferCommit,
    admission,
  ) {
    const attemptOrdinal =
      this.reservationOwner.begin(admission);
    const {
      batchIndex,
      replacement,
      reservations,
    } = admission;
    const lifecycle = new S2CaptureLifecycle();
    lifecycle.enter("Reserved");
    let proofDeadline = state.deadline;
    let batch;
    let launched;
    let stdout;
    let stderr;
    let stdin;
    let handoffOwned = false;
    let proofInstalled = false;
    let proofReservation;
    let retirementComplete = false;
    let finalizerConsumed = false;
    let outputFaultSelected = false;
    let firstFault;
    const secondaryFaults = [];
    try {
      proofDeadline = state.deadline.sub(
        `s2-proof-${String(ordinal).padStart(2, "0")}`,
        DEADLINE_MS.ps,
      );
      checkCanary(this.canary, proofDeadline);
      batch = await this.fifo.create(
        batchIndex,
        ["stdout.fifo", "stderr.fifo"],
        proofDeadline.sub("fifo", DEADLINE_MS.fifoBatch),
      );
      handoffOwned = true;
      lifecycle.enter("AttemptMaterialized");
      stdout = channelReader(
        batch.endpoints[0].reader,
        "S2 ps stdout",
        4_096,
        proofDeadline,
      );
      stderr = channelReader(
        batch.endpoints[1].reader,
        "S2 ps stderr",
        512,
        proofDeadline,
      );
      stdin = openDevNull(fsConstants.O_RDONLY, proofDeadline);
      try {
        requireProtocolPeak(S2_DESCRIPTOR_CAPACITY);
        launched = spawnExact({
          executable: PS,
          args: [
            "-ww",
            "-g",
            String(state.pgid),
            "-o",
            "pid=,ppid=,pgid=,sess=,uid=,state=,lstart=,ucomm=",
          ],
          cwd: batch.batchPath,
          env: closedEnvironment(
            state.roots.receipts.home.path,
            state.roots.receipts.tmp.path,
          ),
          stdio: [
            stdin.fd,
            batch.endpoints[0].writer,
            batch.endpoints[1].writer,
          ],
          label: `s2-${state.leg}-ps-${String(ordinal).padStart(2, "0")}`,
          tombstones: this.tombstones,
          onSpawn: (provisional) => {
            launched = provisional;
          },
        });
      } finally {
        const stdinToClose = stdin;
        stdin = undefined;
        if (stdinToClose !== undefined) {
          checkedClose(stdinToClose.fd, proofDeadline);
        }
      }
      lifecycle.enter("CaptureLaunched");
      handoffOwned = false;
      closeHandoff(batch);
      const [settledOutcome, stdoutOutcome, stderrOutcome] =
        await Promise.all([
          nonthrowingOutcome(
            settleDirectChild(launched, proofDeadline, {
              normalMs: 1_000,
              termMs: 250,
              killMs: 250,
              label: launched.label,
            }),
          ),
          nonthrowingOutcome(stdout.promise),
          nonthrowingOutcome(stderr.promise),
        ]);
      outputFaultSelected =
        settledOutcome.kind === "FAULT" ||
        stdoutOutcome.kind === "FAULT" ||
        stderrOutcome.kind === "FAULT";
      requireCondition(
        settledOutcome.kind === "VALUE" &&
          stdoutOutcome.kind === "VALUE" &&
          stderrOutcome.kind === "VALUE",
        "host.capture_attempt",
        "S2 capture child/output outcome faulted",
        {
          settled:
            settledOutcome.kind === "FAULT"
              ? safeError(settledOutcome.error)
              : "VALUE",
          stdout:
            stdoutOutcome.kind === "FAULT"
              ? safeError(stdoutOutcome.error)
              : "VALUE",
          stderr:
            stderrOutcome.kind === "FAULT"
              ? safeError(stderrOutcome.error)
              : "VALUE",
        },
      );
      const settled = settledOutcome.value;
      const stdoutResult = stdoutOutcome.value;
      const stderrResult = stderrOutcome.value;
      requireCondition(
        settled.terminal.error === null &&
          settled.terminal.code === 0 &&
          settled.terminal.signal === null &&
          stdoutResult.eof &&
          stderrResult.eof &&
          stderrResult.bytes === 0,
        "host.ps_output",
        "S2 ps status/EOF/stderr differs",
        {
          terminal: settled.terminal,
          stdout: captureFacts(stdoutResult),
          stderr: captureFacts(stderrResult),
        },
      );
      const rows = parsePsRows(stdoutResult.body);
      const incarnation = s2ValidateProductionRows(
        state,
        rows,
        ordinal,
      );
      requireCondition(
        predicate(rows),
        "host.proof_predicate",
        `S2 proof predicate ${label} failed`,
        { leg: state.leg, ordinal, rows },
      );
      proofReservation = this.worker.capacity.reserve("proofs");
      const proof = Object.freeze({
        leg: state.leg,
        ordinal,
        capturePid: launched.pid,
        pgidObserved: state.pgid,
        rows,
        terminal: settled.terminal,
        stdout: Object.freeze({
          ...captureFacts(stdoutResult),
          base64: stdoutResult.body.toString("base64"),
        }),
        stderr: Object.freeze({
          ...captureFacts(stderrResult),
          base64: stderrResult.body.toString("base64"),
        }),
        attempt: Object.freeze({
          ordinal: attemptOrdinal,
          batchIndex,
          replacement,
        }),
      });
      lifecycle.enter("ProofInstalled");
      proofInstalled = true;
      this.tombstones.add(launched.pid, "S2_PS_DIRECT_REAP");
      const finishProof = () => {
        requireCondition(
          !finalizerConsumed,
          "host.capture_attempt",
          "S2 proof finalizer was consumed twice",
          { leg: state.leg, ordinal },
        );
        finalizerConsumed = true;
        const finishFaults = [];
        try {
          this.fifo.retire(batch, proofDeadline);
          batch = undefined;
          lifecycle.enter("Retired");
          retirementComplete = true;
        } catch (error) {
          lifecycle.enter("RetirementFault");
          finishFaults.push(error);
        }
        if (finishFaults.length > 0) {
          throw new HostAuthorityError(
            "host.capture_retirement",
            "S2 proof retirement failed",
            {
              firstFault: safeError(finishFaults[0]),
              secondaryFaults: finishFaults
                .slice(1)
                .map(safeError),
              states: lifecycle.snapshot(),
              replacementEligible: false,
              retained: true,
            },
          );
        }
        let record;
        try {
          record = this.evidence.add(
            "ps",
            "ruby.proof",
            {
              ...proof,
              attemptStates: Object.freeze([
                ...lifecycle.snapshot(),
                "EvidenceCommitted",
              ]),
            },
            proofDeadline,
          );
          lifecycle.enter("EvidenceCommitted");
        } catch (error) {
          lifecycle.enter("EvidenceCommitFault");
          throw new HostAuthorityError(
            "host.capture_evidence",
            "S2 proof evidence commit failed",
            {
              firstFault: safeError(error),
              secondaryFaults: [],
              states: lifecycle.snapshot(),
              replacementEligible: false,
              retained: true,
            },
          );
        }
        this.worker.capacity.complete(reservations.attempt);
        this.worker.capacity.complete(reservations.ps);
        this.worker.capacity.complete(proofReservation);
        state.consumed.add(ordinal);
        state.lastOrdinal = ordinal;
        if (state.incarnation === undefined) {
          state.incarnation = incarnation;
        }
        state.proofs.set(ordinal, proof);
        return Object.freeze({ proof, record });
      };
      if (deferCommit) {
        return Object.freeze({
          proof,
          commit: finishProof,
        });
      }
      return finishProof();
    } catch (error) {
      firstFault =
        error instanceof HostAuthorityError
          ? error
          : new HostAuthorityError(
              proofInstalled
                ? "host.capture_attempt"
                : batch === undefined
                  ? "host.capture_materialization"
                  : launched === undefined
                    ? "host.capture_launch"
                    : "host.capture_attempt",
              "S2 capture attempt failed",
              safeError(error),
            );
      if (stdin !== undefined) {
        const stdinToClose = stdin;
        stdin = undefined;
        try {
          checkedClose(stdinToClose.fd, proofDeadline);
        } catch (cleanupError) {
          secondaryFaults.push(cleanupError);
        }
      }
      if (batch !== undefined && handoffOwned) {
        handoffOwned = false;
        try {
          closeHandoff(batch);
        } catch (cleanupError) {
          secondaryFaults.push(cleanupError);
        }
      }
      if (
        launched !== undefined &&
        launched.terminal.current() === null
      ) {
        try {
          await settleDirectChild(launched, proofDeadline, {
            normalMs: 1,
            termMs: 250,
            killMs: 250,
            label: `${launched.label}-fault-closeout`,
            allowSignal: true,
          });
        } catch (cleanupError) {
          secondaryFaults.push(cleanupError);
        }
      }
      for (const reader of [stdout, stderr]) {
        if (reader !== undefined) {
          const outcome = await nonthrowingOutcome(reader.promise);
          if (
            outcome.kind === "FAULT" &&
            !outputFaultSelected
          ) {
            secondaryFaults.push(outcome.error);
          }
        }
      }
      if (
        launched !== undefined &&
        launched.terminal.current() !== null &&
        !this.tombstones.has(launched.pid)
      ) {
        try {
          this.tombstones.add(
            launched.pid,
            "S2_PS_FAULT_DIRECT_REAP",
          );
        } catch (cleanupError) {
          secondaryFaults.push(cleanupError);
        }
      }
      if (!retirementComplete && batch !== undefined) {
        try {
          this.fifo.retire(batch, proofDeadline);
          lifecycle.enter("Retired");
          retirementComplete = true;
        } catch (cleanupError) {
          lifecycle.enter("RetirementFault");
          secondaryFaults.push(cleanupError);
        }
      } else if (!retirementComplete) {
        lifecycle.enter("RetiredNoObject");
        retirementComplete = true;
      }
      let faultEvidenceCommitted = false;
      if (
        !proofInstalled &&
        retirementComplete &&
        secondaryFaults.length === 0
      ) {
        try {
          this.evidence.add(
            "ps",
            "capture.attempt_fault",
            {
              leg: state.leg,
              ordinal,
              attempt: attemptOrdinal,
              batchIndex,
              replacement,
              firstFault: safeError(firstFault),
              states: Object.freeze([
                ...lifecycle.snapshot(),
                "EvidenceCommitted",
              ]),
            },
            proofDeadline,
          );
          lifecycle.enter("EvidenceCommitted");
          this.worker.capacity.complete(reservations.attempt);
          this.worker.capacity.complete(reservations.ps);
          faultEvidenceCommitted = true;
        } catch (evidenceError) {
          lifecycle.enter("EvidenceCommitFault");
          secondaryFaults.push(evidenceError);
        }
      }
      const replacementEligible =
        !proofInstalled &&
        !replacement &&
        retirementComplete &&
        faultEvidenceCommitted &&
        secondaryFaults.length === 0;
      const typed = s2CaptureTypedFault(
        firstFault,
        secondaryFaults,
        lifecycle,
        replacementEligible,
      );
      throw typed;
    }
  }
}

function s2ProofCommandFields(result) {
  const { proof } = result;
  return Object.freeze([
    proof.stdout.base64,
    String(proof.stdout.bytes),
    proof.stdout.sha256,
    String(proof.terminal.code),
    proof.stderr.base64,
    String(proof.stderr.bytes),
    proof.stderr.sha256,
    proof.stdout.eof ? "1" : "0",
    proof.stderr.eof ? "1" : "0",
  ]);
}

function s2RequireToken(token) {
  requireCondition(
    typeof token === "string" &&
      /^[a-f0-9]{32}$/u.test(token),
    "host.s2_protocol",
    "S2 generation token differs",
  );
  return token;
}

function s2ParseReady(frame) {
  const parsed = parseS2SupervisorFrame(frame);
  requireCondition(
    parsed.sequence === 0 &&
      parsed.kind === "READY" &&
      parsed.fields.length === 6,
    "host.s2_protocol",
    "S2 READY shape differs",
    { fields: parsed.fields },
  );
  const tokens = parsed.fields.slice(2).map(s2RequireToken);
  requireCondition(
    new Set(tokens).size === 4,
    "host.s2_protocol",
    "S2 READY tokens are not distinct",
  );
  return Object.freeze({
    ...parsed,
    tokens: Object.freeze(tokens),
  });
}

function s2IntentProof(result) {
  if (result === undefined) return undefined;
  const fields = s2ProofCommandFields(result);
  return Object.freeze({
    stdoutBase64: fields[0],
    stdoutBytes: fields[1],
    stdoutSha256: fields[2],
    terminalCode: fields[3],
    stderrBase64: fields[4],
    stderrBytes: fields[5],
    stderrSha256: fields[6],
    stdoutEof: fields[7],
    stderrEof: fields[8],
  });
}

function s2ProofFieldsFromIntent(intent) {
  const proof = intent.proof;
  requireCondition(
    proof !== null &&
      typeof proof === "object" &&
      Object.keys(proof).sort().join(",") ===
        [
          "stderrBase64",
          "stderrBytes",
          "stderrEof",
          "stderrSha256",
          "stdoutBase64",
          "stdoutBytes",
          "stdoutEof",
          "stdoutSha256",
          "terminalCode",
        ].join(","),
    "host.s2_protocol",
    "S2 proof intent fields differ",
  );
  return Object.freeze([
    proof.stdoutBase64,
    proof.stdoutBytes,
    proof.stdoutSha256,
    proof.terminalCode,
    proof.stderrBase64,
    proof.stderrBytes,
    proof.stderrSha256,
    proof.stdoutEof,
    proof.stderrEof,
  ]);
}

const S2_TRANSITION_KINDS = Object.freeze([
  "LEADER_KILL",
  "SELECTED_SUFFIX_ALREADY_STOPPED",
  "GROUP_CONT",
  "GROUP_TERM",
  "TERMINAL_KILL_PROBE_13",
  "TERMINAL_KILL_PROBE_14",
]);

const S2_INTENT_KEYS = Object.freeze([
  "ack",
  "kind",
  "leg",
  "proof",
  "schema",
  "sequence",
  "token",
]);
const S2_ACK_KEYS = Object.freeze([
  "envelopeSha256",
  "evidenceHash",
  "evidenceSequence",
]);
const S2_ENVELOPE_KEYS = Object.freeze([
  "kind",
  "schema",
  "sequence",
  "supervisorCommandBase64",
  "supervisorCommandSha256",
  "supervisorResultBase64",
  "supervisorResultSha256",
  "workerIntentBase64",
]);
const S2_FINAL_ACK_KEYS = Object.freeze([
  "envelopeSha256",
  "evidenceHash",
  "evidenceSequence",
  "kind",
  "schema",
  "sequence",
]);
const S2_NODE_STOP_KEYS = Object.freeze([
  "faultCode",
  "kind",
  "readerFault",
  "resultEof",
  "schema",
  "secondaries",
  "sequence",
  "stopKind",
  "supervisorResult",
  "supervisorTerminal",
  "workerIntentSha256",
]);
const S2_NODE_STOP_TERMINAL_KEYS = Object.freeze([
  "code",
  "error",
  "signal",
]);

function requireS2NodeStopShape(envelope, label) {
  requireExactKeys(
    envelope,
    S2_NODE_STOP_KEYS,
    "host.s2_protocol",
    `${label} NODE_STOP`,
  );
  requireExactKeys(
    envelope.supervisorTerminal,
    S2_NODE_STOP_TERMINAL_KEYS,
    "host.s2_protocol",
    `${label} NODE_STOP terminal`,
  );
  const supervisorResult = decodeS2RawFact(
    envelope.supervisorResult,
    `${label} NODE_STOP supervisor result`,
    S2_PROTOCOL.frameBytes,
  );
  decodeS2RawFact(
    envelope.readerFault,
    `${label} NODE_STOP reader fault`,
    S2_PROTOCOL.frameBytes,
  );
  validateS2SecondaryFacts(
    envelope.secondaries,
    `${label} NODE_STOP`,
  );
  requireCondition(
    envelope.schema === 1 &&
      envelope.kind === "NODE_STOP" &&
      Number.isSafeInteger(envelope.sequence) &&
      envelope.sequence >= 0 &&
      envelope.sequence <=
        S2_PROTOCOL.supervisorOutputFrames &&
      (S2_SUPERVISOR_STOP_KINDS.includes(envelope.stopKind) ||
        envelope.stopKind === "SUPERVISOR_TRANSPORT_STOP" ||
        envelope.stopKind === "SUPERVISOR_DEATH_STOP") &&
      /^[A-Z0-9_.-]{1,80}$/u.test(envelope.faultCode) &&
      typeof envelope.resultEof === "boolean" &&
      /^[a-f0-9]{64}$/u.test(
        envelope.workerIntentSha256,
      ) &&
      (envelope.supervisorTerminal.code === null ||
        Number.isInteger(envelope.supervisorTerminal.code)) &&
      (envelope.supervisorTerminal.signal === null ||
        typeof envelope.supervisorTerminal.signal === "string") &&
      (envelope.supervisorTerminal.error === null ||
        typeof envelope.supervisorTerminal.error === "object") &&
      (supervisorResult.length === 0 ||
        parseS2SupervisorStop(supervisorResult) !== null),
    "host.s2_protocol",
    `${label} NODE_STOP values differ`,
  );
}

function s2ExpectedTransitionOutcome(kind) {
  if (kind === "SELECTED_SUFFIX_ALREADY_STOPPED") {
    return "MARKED";
  }
  if (
    kind === "TERMINAL_KILL_PROBE_13" ||
    kind === "TERMINAL_KILL_PROBE_14"
  ) {
    return "NO_SIGNALABLE_GROUP_MEMBERS";
  }
  requireCondition(
    kind === "LEADER_KILL" ||
      kind === "GROUP_CONT" ||
      kind === "GROUP_TERM",
    "host.s2_protocol",
    "S2 transition kind has no exact outcome",
    { kind },
  );
  return "SIGNALED";
}

function s2ExpectedTransitionOrdinal(leg, kind) {
  if (
    kind === "TERMINAL_KILL_PROBE_13" ||
    kind === "TERMINAL_KILL_PROBE_14"
  ) {
    return kind.endsWith("_13") ? 13 : 14;
  }
  if (leg === "support" || leg === "denial") {
    requireCondition(
      kind === "GROUP_CONT",
      "host.s2_protocol",
      "simple leg transition kind differs",
      { leg, kind },
    );
    return 3;
  }
  const ordinals = Object.freeze({
    LEADER_KILL: 4,
    SELECTED_SUFFIX_ALREADY_STOPPED: 5,
    GROUP_CONT: 6,
    GROUP_TERM: 10,
  });
  requireCondition(
    (leg === "one-receipt" || leg === "parent-loss") &&
      Object.hasOwn(ordinals, kind),
    "host.s2_protocol",
    "complex leg transition ordinal differs",
    { leg, kind },
  );
  return ordinals[kind];
}

function requireS2Ack(ack, label) {
  requireExactKeys(ack, S2_ACK_KEYS, "host.s2_protocol", `${label} ack`);
  requireCondition(
    Number.isSafeInteger(ack.evidenceSequence) &&
      ack.evidenceSequence >= 0 &&
      /^[a-f0-9]{64}$/u.test(ack.evidenceHash) &&
      /^[a-f0-9]{64}$/u.test(ack.envelopeSha256),
    "host.s2_protocol",
    `${label} ack values differ`,
  );
}

function requireS2IntentShape(intent, label) {
  requireExactKeys(
    intent,
    S2_INTENT_KEYS,
    "host.s2_protocol",
    `${label} intent`,
  );
  requireS2Ack(intent.ack, label);
  requireCondition(
    intent.schema === 1 &&
      Number.isSafeInteger(intent.sequence) &&
      intent.sequence >= 0 &&
      intent.sequence < S2_PROTOCOL.supervisorInputFrames &&
      typeof intent.kind === "string",
    "host.s2_protocol",
    `${label} intent header differs`,
  );
  if (intent.kind === "START_LEG") {
    requireCondition(
      S2_LEGS.includes(intent.leg) &&
        intent.proof === null &&
        typeof intent.token === "string",
      "host.s2_protocol",
      `${label} START_LEG shape differs`,
    );
  } else if (S2_TRANSITION_KINDS.includes(intent.kind)) {
    requireCondition(
      intent.leg === null &&
        intent.proof !== null &&
        typeof intent.token === "string",
      "host.s2_protocol",
      `${label} transition shape differs`,
    );
  } else if (intent.kind === "FINAL_REAP") {
    requireCondition(
      intent.leg === null &&
        intent.proof === null &&
        typeof intent.token === "string",
      "host.s2_protocol",
      `${label} FINAL_REAP shape differs`,
    );
  } else if (intent.kind === "CLOSE") {
    requireCondition(
      intent.leg === null &&
        intent.proof === null &&
        intent.token === null &&
        intent.sequence === 26,
      "host.s2_protocol",
      `${label} CLOSE shape differs`,
    );
  } else {
    fail("host.s2_protocol", `${label} intent kind is unknown`, {
      kind: intent.kind,
    });
  }
}

function requireS2EnvelopeShape(envelope, label) {
  requireExactKeys(
    envelope,
    S2_ENVELOPE_KEYS,
    "host.s2_protocol",
    `${label} envelope`,
  );
  requireCondition(
    envelope.schema === 1 &&
      Number.isSafeInteger(envelope.sequence) &&
      typeof envelope.kind === "string",
    "host.s2_protocol",
    `${label} envelope header differs`,
  );
}

function requireS2FinalAckShape(ack, label) {
  requireExactKeys(
    ack,
    S2_FINAL_ACK_KEYS,
    "host.s2_protocol",
    `${label} final ACK`,
  );
  requireCondition(
    ack.schema === 1 &&
      ack.sequence === 27 &&
      ack.kind === "COMMIT_ACK" &&
      Number.isSafeInteger(ack.evidenceSequence) &&
      ack.evidenceSequence >= 0 &&
      /^[a-f0-9]{64}$/u.test(ack.evidenceHash) &&
      /^[a-f0-9]{64}$/u.test(ack.envelopeSha256),
    "host.s2_protocol",
    `${label} final ACK values differ`,
  );
}

function s2SupervisorCommand(intent) {
  requireS2IntentShape(intent, "S2 worker");
  let fields;
  if (intent.kind === "START_LEG") {
    requireCondition(
      S2_LEGS.includes(intent.leg) &&
        intent.proof === null,
      "host.s2_protocol",
      "S2 START_LEG intent differs",
    );
    fields = [
      String(intent.sequence),
      intent.kind,
      s2RequireToken(intent.token),
      intent.leg,
    ];
  } else if (S2_TRANSITION_KINDS.includes(intent.kind)) {
    fields = [
      String(intent.sequence),
      intent.kind,
      s2RequireToken(intent.token),
      ...s2ProofFieldsFromIntent(intent),
    ];
  } else if (intent.kind === "FINAL_REAP") {
    requireCondition(
      intent.proof === null,
      "host.s2_protocol",
      "S2 FINAL_REAP carried proof payload",
    );
    fields = [
      String(intent.sequence),
      intent.kind,
      s2RequireToken(intent.token),
    ];
  } else if (intent.kind === "CLOSE") {
    requireCondition(
      intent.sequence === 26 &&
        intent.token === null &&
        intent.proof === null,
      "host.s2_protocol",
      "S2 CLOSE intent differs",
    );
    fields = [String(intent.sequence), intent.kind];
  } else {
    fail(
      "host.s2_protocol",
      "unknown S2 worker intent",
      { kind: intent.kind },
    );
  }
  const frame = Buffer.from(`${fields.join("|")}\n`, "ascii");
  requireCondition(
    frame.length <= S2_PROTOCOL.frameBytes,
    "host.s2_protocol",
    "S2 supervisor command exceeded its frame bound",
    { bytes: frame.length, kind: intent.kind },
  );
  return frame;
}

function s2ValidateSupervisorResult(
  parsed,
  intent,
  expectedOutputSequence,
  expectedLeg = undefined,
) {
  const expectedKind =
    intent.kind === "START_LEG"
      ? "LEADER_OWNED"
      : S2_TRANSITION_KINDS.includes(intent.kind)
        ? "TRANSITION"
        : intent.kind === "FINAL_REAP"
          ? "LEADER_REAPED"
          : "CLOSEOUT";
  requireCondition(
    parsed.sequence === expectedOutputSequence &&
      parsed.kind === expectedKind,
    "host.s2_protocol",
    "S2 supervisor result kind/sequence differs",
    {
      expectedOutputSequence,
      expectedKind,
      actualSequence: parsed.sequence,
      actualKind: parsed.kind,
    },
  );
  if (expectedKind === "LEADER_OWNED") {
    requireCondition(
      parsed.fields.length === 7 &&
        parsed.fields[2] === intent.token &&
        parsed.fields[5] === "1" &&
        parsed.fields[6] === "1",
      "host.s2_protocol",
      "S2 LEADER_OWNED result differs",
    );
    const report = Buffer.from(parsed.fields[3], "base64");
    const release = Buffer.from(parsed.fields[4], "base64");
    requireCondition(
      report.toString("base64") === parsed.fields[3] &&
        release.toString("base64") === parsed.fields[4] &&
        report.toString("ascii") ===
          "SETSID_OK\nEXEC_OK\nRELEASED_OK\n" &&
        release.toString("ascii") === "RELEASE_OK\n",
      "host.s2_protocol",
      "S2 startup transcript differs",
    );
  } else if (expectedKind === "TRANSITION") {
    const expectedOutcome = s2ExpectedTransitionOutcome(intent.kind);
    requireCondition(
      parsed.fields.length === 5 &&
        parsed.fields[2] === intent.token &&
        parsed.fields[3] === intent.kind &&
        parsed.fields[4] === expectedOutcome,
      "host.s2_protocol",
      "S2 transition result differs",
      {
        kind: intent.kind,
        expectedOutcome,
        actualOutcome: parsed.fields[4],
      },
    );
  } else if (expectedKind === "LEADER_REAPED") {
    const expectedTerminal =
      expectedLeg === "support" || expectedLeg === "denial"
        ? ["EXITED", "0"]
        : expectedLeg === "one-receipt" ||
            expectedLeg === "parent-loss"
          ? ["SIGNALED", "9"]
          : null;
    requireCondition(
      expectedTerminal !== null &&
      parsed.fields.length === 5 &&
        parsed.fields[2] === intent.token &&
        parsed.fields[3] === expectedTerminal[0] &&
        parsed.fields[4] === expectedTerminal[1],
      "host.s2_protocol",
      "S2 LEADER_REAPED result differs",
      {
        expectedLeg,
        expectedTerminal,
        actualTerminal: parsed.fields.slice(3),
      },
    );
  } else {
    requireCondition(
      parsed.fields.length === 2 &&
        intent.kind === "CLOSE",
      "host.s2_protocol",
      "S2 CLOSEOUT result differs",
    );
  }
}

class S2RelayClient {
  constructor(stream, deadline) {
    this.stream = stream;
    this.deadline = deadline;
    this.reader = new S2FrameReader(
      stream,
      "worker relay result",
      new S2TransportBudget(
        "worker relay result",
        S2_PROTOCOL.relayEnvelopes,
        S2_PROTOCOL.relayEnvelopeBytes,
        S2_PROTOCOL.resultEnvelopeBytes,
      ),
      deadline,
    );
    this.writerBudget = new S2TransportBudget(
      "worker relay intent",
      S2_PROTOCOL.relayFrames,
      S2_PROTOCOL.relayFrameBytes,
    );
    this.intentSequence = 0;
    this.expectedEnvelopeSequence = 0;
    this.pendingAck = null;
    this.closed = false;
  }

  async receiveReady() {
    const rawEnvelope = await this.reader.read();
    requireCondition(
      rawEnvelope !== null,
      "host.s2_protocol",
      "worker relay ended before READY",
    );
    const envelope = parseS2JsonFrame(
      rawEnvelope,
      "worker READY envelope",
    );
    requireS2EnvelopeShape(envelope, "worker READY");
    requireCondition(
      envelope.schema === 1 &&
        envelope.sequence === 0 &&
        envelope.kind === "READY" &&
        envelope.workerIntentBase64 === null &&
        envelope.supervisorCommandBase64 === null &&
        envelope.supervisorCommandSha256 === null &&
        typeof envelope.supervisorResultBase64 === "string",
      "host.s2_protocol",
      "worker READY envelope shape differs",
    );
    const supervisorResult = Buffer.from(
      envelope.supervisorResultBase64,
      "base64",
    );
    requireCondition(
      supervisorResult.toString("base64") ===
          envelope.supervisorResultBase64 &&
        envelope.supervisorResultSha256 ===
          sha256(supervisorResult),
      "host.s2_protocol",
      "worker READY result base64 is noncanonical",
    );
    const ready = s2ParseReady(supervisorResult);
    this.expectedEnvelopeSequence = 1;
    return Object.freeze({
      envelope,
      rawEnvelope,
      supervisorResult,
      ready,
    });
  }

  acknowledge(record, rawEnvelope) {
    requireCondition(
      this.pendingAck === null &&
        record !== null &&
        typeof record.hash === "string" &&
        Buffer.isBuffer(rawEnvelope),
      "host.s2_protocol",
      "worker relay acknowledgement owner differs",
    );
    this.pendingAck = Object.freeze({
      evidenceSequence: record.sequence,
      evidenceHash: record.hash,
      envelopeSha256: sha256(rawEnvelope),
    });
  }

  async command(
    kind,
    token,
    leg,
    proof = undefined,
    expectedLeg = leg,
  ) {
    requireCondition(
      this.pendingAck !== null &&
        this.intentSequence <
          S2_PROTOCOL.supervisorInputFrames,
      "host.s2_protocol",
      "worker command lacks committed preceding envelope",
    );
    const intent = Object.freeze({
      schema: 1,
      sequence: this.intentSequence,
      kind,
      token,
      leg: leg ?? null,
      proof: s2IntentProof(proof) ?? null,
      ack: this.pendingAck,
    });
    const rawIntent = encodeS2JsonFrame(
      intent,
      S2_PROTOCOL.frameBytes,
      "worker intent",
    );
    await writeS2Frame(
      this.stream,
      rawIntent,
      "worker relay intent",
      this.writerBudget,
      this.deadline,
    );
    this.pendingAck = null;
    const rawEnvelope = await this.reader.read();
    requireCondition(
      rawEnvelope !== null,
      "host.s2_protocol",
      "worker relay ended before command result",
    );
    const envelope = parseS2JsonFrame(
      rawEnvelope,
      "worker result envelope",
    );
    if (envelope.kind === "NODE_STOP") {
      requireS2NodeStopShape(envelope, "worker");
      requireCondition(
        envelope.sequence ===
            this.expectedEnvelopeSequence &&
          envelope.workerIntentSha256 === sha256(rawIntent),
        "host.s2_protocol",
        "worker NODE_STOP intent relation differs",
      );
      throw new HostAuthorityError(
        "host.s2_node_stop",
        "custody supervisor/transport selected retained STOP",
        Object.freeze({
          nodeStop: envelope,
          workerIntent: s2RawFact(rawIntent),
          supervisorCommand: s2RawFact(
            s2SupervisorCommand(intent),
          ),
          nodeEnvelope: s2RawFact(rawEnvelope),
        }),
      );
    }
    requireS2EnvelopeShape(envelope, "worker result");
    requireCondition(
      envelope.schema === 1 &&
        envelope.sequence ===
          this.expectedEnvelopeSequence &&
        envelope.kind === kind &&
        envelope.workerIntentBase64 ===
          rawIntent.toString("base64"),
      "host.s2_protocol",
      "worker result envelope relation differs",
    );
    const supervisorCommand = Buffer.from(
      envelope.supervisorCommandBase64,
      "base64",
    );
    const supervisorResult = Buffer.from(
      envelope.supervisorResultBase64,
      "base64",
    );
    requireCondition(
      supervisorCommand.toString("base64") ===
        envelope.supervisorCommandBase64 &&
        supervisorResult.toString("base64") ===
          envelope.supervisorResultBase64 &&
        sha256(supervisorCommand) ===
          envelope.supervisorCommandSha256 &&
        sha256(supervisorResult) ===
          envelope.supervisorResultSha256,
      "host.s2_protocol",
      "worker result envelope raw-byte relation differs",
    );
    const parsedResult =
      parseS2SupervisorFrame(supervisorResult);
    s2ValidateSupervisorResult(
      parsedResult,
      intent,
      this.expectedEnvelopeSequence,
      expectedLeg,
    );
    this.intentSequence += 1;
    this.expectedEnvelopeSequence += 1;
    return Object.freeze({
      intent,
      rawIntent,
      envelope,
      rawEnvelope,
      supervisorCommand,
      supervisorResult,
      parsedResult,
    });
  }

  async finalAck(record, rawEnvelope) {
    this.acknowledge(record, rawEnvelope);
    requireCondition(
      this.intentSequence ===
        S2_PROTOCOL.supervisorInputFrames &&
        this.expectedEnvelopeSequence ===
          S2_PROTOCOL.supervisorOutputFrames &&
        this.pendingAck !== null,
      "host.s2_protocol",
      "worker final ACK occurred before clean protocol closure",
    );
    const ack = Object.freeze({
      schema: 1,
      sequence: 27,
      kind: "COMMIT_ACK",
      evidenceSequence: record.sequence,
      evidenceHash: record.hash,
      envelopeSha256: this.pendingAck.envelopeSha256,
    });
    const rawAck = encodeS2JsonFrame(
      ack,
      S2_PROTOCOL.frameBytes,
      "worker final ACK",
    );
    await writeS2Frame(
      this.stream,
      rawAck,
      "worker final ACK",
      this.writerBudget,
      this.deadline,
    );
    this.pendingAck = null;
    this.stream.end();
    this.closed = true;
    return Object.freeze({ ack, rawAck });
  }
}

function s2RelayEvidenceFacts(exchange) {
  return Object.freeze({
    workerIntent: s2RawFact(exchange.rawIntent),
    supervisorCommand: s2RawFact(
      exchange.supervisorCommand,
    ),
    supervisorResult: s2RawFact(exchange.supervisorResult),
    nodeEnvelope: s2RawFact(exchange.rawEnvelope),
  });
}

function s2CommitExchange(
  evidence,
  relay,
  partition,
  kind,
  facts,
  exchange,
  deadline,
) {
  const record = evidence.add(
    partition,
    kind,
    Object.freeze({
      ...facts,
      relay: s2RelayEvidenceFacts(exchange),
    }),
    deadline,
  );
  relay.acknowledge(record, exchange.rawEnvelope);
  return record;
}

function s2InitialHeldRows(rows, pgid) {
  return (
    rows.length === 2 &&
    rows.every(
      (row) =>
        row.pgid === pgid &&
        isStopped(row) &&
        !isZombie(row),
    ) &&
    rows.some((row) => row.pid === pgid)
  );
}

function s2LeaderAndStoppedChildRows(rows, pgid) {
  return (
    rows.length === 2 &&
    rows.some(
      (row) => row.pid === pgid && isZombie(row),
    ) &&
    rows.some(
      (row) =>
        row.pid !== pgid &&
        isStopped(row) &&
        !isZombie(row),
    )
  );
}

function s2LeaderAndLiveChildRows(rows, pgid) {
  return (
    rows.length === 2 &&
    rows.some(
      (row) => row.pid === pgid && isZombie(row),
    ) &&
    rows.some(
      (row) =>
        row.pid !== pgid &&
        !isStopped(row) &&
        !isZombie(row),
    )
  );
}

function s2ZombieLeaderOnly(rows, pgid) {
  return (
    rows.length === 1 &&
    rows[0].pid === pgid &&
    rows[0].pgid === pgid &&
    isZombie(rows[0])
  );
}

function s2ValidateProductionRows(state, rows, ordinal) {
  const topology = state.topology;
  requireCondition(
    topology !== undefined &&
      topology.parent !== undefined &&
      topology.parent.pid === topology.pgid &&
      state.pgid === topology.pgid,
    "host.ruby_identity",
    "S2 proof state lacks receipt-bound leader authority",
  );
  const leader = rows.find(
    (row) => row.pid === topology.parent.pid,
  );
  const childRows = rows.filter(
    (row) => row.pid !== topology.parent.pid,
  );
  const establishedChild =
    topology.child ??
    (state.incarnation === undefined
      ? childRows.length === 1
        ? Object.freeze({
            pid: childRows[0].pid,
            ppid: topology.parent.pid,
            pgid: topology.pgid,
            sid: topology.pgid,
          })
        : undefined
      : state.incarnation.child);
  requireCondition(
    leader !== undefined &&
      leader.ppid === topology.parent.ppid &&
      leader.pgid === topology.pgid &&
      leader.sessObservedZero === "0" &&
      leader.uid === HOST_UID &&
      leader.ucomm === "ruby" &&
      establishedChild !== undefined &&
      establishedChild.pid !== leader.pid &&
      (topology.child === undefined ||
        (establishedChild.pid === topology.child.pid &&
          establishedChild.ppid === topology.child.ppid &&
          establishedChild.pgid === topology.child.pgid &&
          establishedChild.sid === topology.child.sid)),
    "host.ruby_identity",
    "S2 receipt and proof identities do not bind one generation",
    { leg: state.leg, ordinal, rows, topology },
  );
  if (state.incarnation !== undefined) {
    requireCondition(
      leader.lstart === state.incarnation.leader.lstart &&
        establishedChild.pid === state.incarnation.child.pid,
      "host.ruby_identity",
      "S2 proof changed a bound PID incarnation",
      { leg: state.leg, ordinal, rows },
    );
    const observedChild = childRows[0];
    if (observedChild !== undefined) {
      requireCondition(
        observedChild.lstart ===
          state.incarnation.child.lstart,
        "host.ruby_identity",
        "S2 child lstart changed within a generation",
        { leg: state.leg, ordinal, observedChild },
      );
    }
  }
  const initial =
    ordinal === 1 ||
    ordinal === 2 ||
    ordinal === 3 ||
    ordinal === 4;
  const stoppedSuffix =
    (state.leg === "one-receipt" ||
      state.leg === "parent-loss") &&
    (ordinal === 5 || ordinal === 6);
  const liveSuffix =
    (state.leg === "one-receipt" ||
      state.leg === "parent-loss") &&
    ordinal >= 7 &&
    ordinal <= 10;
  const terminal =
    ordinal === 13 ||
    ordinal === 14 ||
    ((state.leg === "one-receipt" ||
      state.leg === "parent-loss") &&
      (ordinal === 11 || ordinal === 12));
  const child = childRows[0];
  requireCondition(
    (initial &&
      rows.length === 2 &&
      childRows.length === 1 &&
      child.pid === establishedChild.pid &&
      child.ppid === leader.pid &&
      child.pgid === leader.pgid &&
      child.sessObservedZero === "0" &&
      child.uid === HOST_UID &&
      child.ucomm === "ruby" &&
      isStopped(leader) &&
      !isZombie(leader) &&
      isStopped(child) &&
      !isZombie(child)) ||
      (stoppedSuffix &&
        rows.length === 2 &&
        childRows.length === 1 &&
        child.pid === establishedChild.pid &&
        child.ppid === 1 &&
        child.pgid === leader.pgid &&
        child.sessObservedZero === "0" &&
        child.uid === HOST_UID &&
        child.ucomm === "ruby" &&
        isZombie(leader) &&
        isStopped(child) &&
        !isZombie(child)) ||
      (liveSuffix &&
        rows.length === 2 &&
        childRows.length === 1 &&
        child.pid === establishedChild.pid &&
        child.ppid === 1 &&
        child.pgid === leader.pgid &&
        child.sessObservedZero === "0" &&
        child.uid === HOST_UID &&
        child.ucomm === "ruby" &&
        isZombie(leader) &&
        !isStopped(child) &&
        !isZombie(child)) ||
      (terminal &&
        rows.length === 1 &&
        isZombie(leader)),
    "host.proof_predicate",
    "S2 proof rows do not match the exact leg/ordinal state",
    { leg: state.leg, ordinal, rows },
  );
  return Object.freeze({
    leader: Object.freeze({
      pid: leader.pid,
      ppid: leader.ppid,
      pgid: leader.pgid,
      lstart: leader.lstart,
    }),
    child: Object.freeze({
      pid: establishedChild.pid,
      pgid: establishedChild.pgid,
      lstart:
        state.incarnation?.child.lstart ??
        childRows[0]?.lstart,
    }),
  });
}

function validateS2ReceiptTopology(
  records,
  expectedBranch,
) {
  requireCondition(
    records.length >= 1 &&
      records.length <= 2 &&
      records.every(
        (record) =>
          record.branch === expectedBranch &&
          record.pgid === record.sid,
      ),
    "host.ruby_topology",
    "S2 receipt authority prefix differs",
    { records, expectedBranch },
  );
  const parent = records.find((record) => record.role === "P");
  const child = records.find((record) => record.role === "C");
  if (records.length === 2) {
    requireCondition(
      parent !== undefined &&
        child !== undefined &&
        parent.pid === parent.pgid &&
        parent.sid === parent.pgid &&
        child.pid !== parent.pid &&
        child.ppid === parent.pid &&
        child.pgid === parent.pgid &&
        child.sid === parent.sid,
      "host.ruby_topology",
      "S2 two-role receipt topology differs",
      { records },
    );
  } else {
    requireCondition(
      records[0].role === "P" &&
        records[0].pid === records[0].pgid &&
        records[0].sid === records[0].pgid,
      "host.ruby_topology",
      "S2 one-receipt prefix is not the bound leader",
      { record: records[0] },
    );
  }
  return Object.freeze({
    parent,
    child,
    pgid: records[0].pgid,
  });
}

async function runS2RubyLeg({
  worker,
  fifo,
  proofOwner,
  evidence,
  tombstones,
  relay,
  token,
  legIndex,
  leg,
  containerReceipt,
  previousRoots,
}) {
  const deadline = worker.deadline.sub(
    `s2-ruby-${leg}`,
    DEADLINE_MS.ruby,
  );
  const support = leg !== "denial";
  const reservations = reserveRubyLeg(worker.capacity, support);
  const roots = createRubyRoots(
    worker,
    containerReceipt,
    previousRoots,
  );
  const batch = await fifo.createExternal(
    36 + legIndex,
    ["receipt.fifo", "stdout.fifo", "stderr.fifo"],
    deadline.sub("fifo", DEADLINE_MS.fifoBatch),
  );
  const oneReceipt = leg === "one-receipt";
  const operationLeg =
    leg === "support"
      ? "SUPPORT"
      : leg === "denial"
        ? "DENIAL"
        : leg === "parent-loss"
          ? "PARENT_LOSS"
          : null;
  const receiptReader = rubyReceiptReader(
    batch.endpoints[0].reader,
    deadline,
    oneReceipt ? "ONE_RECEIPT" : "ORDINARY",
    operationLeg,
  );
  const stdout = channelReader(
    batch.endpoints[1].reader,
    "S2 Ruby stdout",
    64,
    deadline,
  );
  const stderr = channelReader(
    batch.endpoints[2].reader,
    "S2 Ruby stderr",
    4_096,
    deadline,
  );
  const start = await relay.command(
    "START_LEG",
    token,
    leg,
  );
  const startRecord = s2CommitExchange(
    evidence,
    relay,
    "supervisor",
    "supervisor.lifecycle",
    {
      lifecycle: "LEADER_OWNED",
      leg,
      legIndex,
      token,
      startupReport: s2RawFact(
        Buffer.from(start.parsedResult.fields[3], "base64"),
      ),
      startupRelease: s2RawFact(
        Buffer.from(start.parsedResult.fields[4], "base64"),
      ),
      reportEof: true,
      releaseEof: true,
    },
    start,
    deadline,
  );
  requireCondition(
    startRecord.kind === "supervisor.lifecycle",
    "host.s2_protocol",
    "S2 LEADER_OWNED evidence did not commit",
  );
  fifo.activateExternal(batch, deadline);
  const receiptTransport = await receiptReader.promise;
  receiptReader.close();
  requireCondition(
    receiptTransport.latch.outcome ===
      (oneReceipt
        ? "ONE_RECEIPT_ELIGIBLE_AFTER_CLEANUP"
        : "ORDINARY_SUCCESS") &&
      receiptTransport.records.length ===
        (oneReceipt ? 1 : 2),
    "host.ruby_receipt_terminal",
    "S2 receipt transport did not select its exact clean prefix",
    {
      leg,
      latch: receiptTransport.latch,
      counters: receiptTransport.counters,
    },
  );
  const topology = validateS2ReceiptTopology(
    receiptTransport.records,
    support ? "S" : "D",
  );
  const state = {
    leg,
    pgid: topology.pgid,
    topology,
    incarnation: undefined,
    deadline,
    roots,
    consumed: new Set(),
    lastOrdinal: 0,
    proofs: new Map(),
  };
  await delay(deadline, 5);
  await proofOwner.capture(
    state,
    1,
    (rows) => s2InitialHeldRows(rows, state.pgid),
    "initial-held",
  );
  await proofOwner.capture(
    state,
    2,
    (rows) => s2InitialHeldRows(rows, state.pgid),
    "confirmed-held",
  );
  let socketIdentities = [];
  if (support) {
    socketIdentities = [
      captureSocketIdentity(
        roots.parentPath,
        roots.receipts.parent,
        topology.parent,
      ),
      captureSocketIdentity(
        roots.childPath,
        roots.receipts.child,
        topology.child,
      ),
    ];
  } else {
    requireCondition(
      absentNoFollow(roots.parentPath) &&
        absentNoFollow(roots.childPath),
      "host.ruby_denial",
      "S2 denial created a socket pathname",
    );
  }
  const transitionRecords = [];
  const issueTransition = async (kind, capture) => {
    let exchange;
    let relayFault;
    try {
      exchange = await relay.command(
        kind,
        token,
        null,
        capture,
        leg,
      );
    } catch (error) {
      relayFault = error;
    }
    let proofCommit;
    let commitFault;
    try {
      proofCommit = capture.commit();
    } catch (error) {
      commitFault = error;
    }
    if (relayFault !== undefined) {
      throw aggregate(
        relayFault,
        commitFault === undefined ? [] : [commitFault],
      );
    }
    if (commitFault !== undefined) throw commitFault;
    requireCondition(
      proofCommit.record.kind === "ruby.proof",
      "host.capture_evidence",
      "signal-adjacent proof did not commit after transition",
    );
    const outcome = exchange.parsedResult.fields[4];
    if (
      kind === "TERMINAL_KILL_PROBE_13" ||
      kind === "TERMINAL_KILL_PROBE_14"
    ) {
      requireCondition(
        outcome === "NO_SIGNALABLE_GROUP_MEMBERS",
        "host.s2_signal",
        "S2 terminal group probe did not return typed absence",
      );
    } else if (
      kind === "SELECTED_SUFFIX_ALREADY_STOPPED"
    ) {
      requireCondition(
        outcome === "MARKED",
        "host.s2_signal",
        "S2 no-syscall marker result differs",
      );
    } else {
      requireCondition(
        outcome === "SIGNALED",
        "host.s2_signal",
        "S2 private signal result differs",
        { kind, outcome },
      );
    }
    const record = s2CommitExchange(
      evidence,
      relay,
      "transitions",
      "supervisor.transition",
      {
        leg,
        legIndex,
        token,
        transition: kind,
        outcome,
        proofOrdinal: capture.proof.ordinal,
      },
      exchange,
      deadline,
    );
    transitionRecords.push(record);
    return exchange;
  };
  let stdoutResult;
  let stderrResult;
  if (leg === "support" || leg === "denial") {
    const proof3 = await proofOwner.capture(
      state,
      3,
      (rows) => s2InitialHeldRows(rows, state.pgid),
      "pre-group-cont",
      true,
    );
    await issueTransition("GROUP_CONT", proof3);
    const [stdoutOutcome, stderrOutcome] = await Promise.all([
      nonthrowingOutcome(stdout.promise),
      nonthrowingOutcome(stderr.promise),
    ]);
    requireCondition(
      stdoutOutcome.kind === "VALUE" &&
        stderrOutcome.kind === "VALUE",
      "host.ruby_output",
      "S2 ordinary Ruby output transport faulted",
      {
        stdout:
          stdoutOutcome.kind === "FAULT"
            ? safeError(stdoutOutcome.error)
            : "VALUE",
        stderr:
          stderrOutcome.kind === "FAULT"
            ? safeError(stderrOutcome.error)
            : "VALUE",
      },
    );
    stdoutResult = stdoutOutcome.value;
    stderrResult = stderrOutcome.value;
    requireCondition(
      stdoutResult.eof &&
        stderrResult.eof &&
        stderrResult.bytes === 0 &&
        stdoutResult.body.toString("ascii") ===
          (leg === "support" ? "SUCCESS\n" : "DENIED\n"),
      "host.ruby_output",
      "S2 ordinary Ruby output differs",
      {
        leg,
        stdout: captureFacts(stdoutResult),
        stderr: captureFacts(stderrResult),
      },
    );
  } else {
    const proof4 = await proofOwner.capture(
      state,
      4,
      (rows) => s2InitialHeldRows(rows, state.pgid),
      "pre-leader-kill",
      true,
    );
    await issueTransition("LEADER_KILL", proof4);
    await delay(deadline, 5);
    const proof5 = await proofOwner.capture(
      state,
      5,
      (rows) =>
        s2LeaderAndStoppedChildRows(rows, state.pgid),
      "leader-zombie-child-stopped",
      true,
    );
    await issueTransition(
      "SELECTED_SUFFIX_ALREADY_STOPPED",
      proof5,
    );
    const proof6 = await proofOwner.capture(
      state,
      6,
      (rows) =>
        s2LeaderAndStoppedChildRows(rows, state.pgid),
      "pre-survivor-cont",
      true,
    );
    await issueTransition("GROUP_CONT", proof6);
    await delay(deadline, 5);
    for (const ordinal of [7, 8, 9]) {
      await proofOwner.capture(
        state,
        ordinal,
        (rows) =>
          s2LeaderAndLiveChildRows(rows, state.pgid),
        `survivor-live-${ordinal}`,
      );
    }
    const proof10 = await proofOwner.capture(
      state,
      10,
      (rows) =>
        s2LeaderAndLiveChildRows(rows, state.pgid),
      "pre-survivor-term",
      true,
    );
    await issueTransition("GROUP_TERM", proof10);
    const [stdoutOutcome, stderrOutcome] = await Promise.all([
      nonthrowingOutcome(stdout.promise),
      nonthrowingOutcome(stderr.promise),
    ]);
    requireCondition(
      stdoutOutcome.kind === "VALUE" &&
        stderrOutcome.kind === "VALUE",
      "host.ruby_fault_output",
      "S2 leader-loss Ruby output transport faulted",
      {
        stdout:
          stdoutOutcome.kind === "FAULT"
            ? safeError(stdoutOutcome.error)
            : "VALUE",
        stderr:
          stderrOutcome.kind === "FAULT"
            ? safeError(stderrOutcome.error)
            : "VALUE",
      },
    );
    stdoutResult = stdoutOutcome.value;
    stderrResult = stderrOutcome.value;
    requireCondition(
      stdoutResult.eof &&
        stderrResult.eof &&
        stdoutResult.bytes === 0 &&
        stderrResult.bytes === 0,
      "host.ruby_fault_output",
      "S2 leader-loss Ruby output differs",
      {
        leg,
        stdout: captureFacts(stdoutResult),
        stderr: captureFacts(stderrResult),
      },
    );
    for (const ordinal of [11, 12]) {
      await proofOwner.capture(
        state,
        ordinal,
        (rows) => s2ZombieLeaderOnly(rows, state.pgid),
        `terminal-leader-${ordinal}`,
      );
    }
  }
  for (const ordinal of [13, 14]) {
    const terminalProof = await proofOwner.capture(
      state,
      ordinal,
      (rows) => s2ZombieLeaderOnly(rows, state.pgid),
      `terminal-probe-${ordinal}`,
      true,
    );
    await issueTransition(
      ordinal === 13
        ? "TERMINAL_KILL_PROBE_13"
        : "TERMINAL_KILL_PROBE_14",
      terminalProof,
    );
  }
  const finalReap = await relay.command(
    "FINAL_REAP",
    token,
    null,
    undefined,
    leg,
  );
  const finalReapRecord = s2CommitExchange(
    evidence,
    relay,
    "supervisor",
    "supervisor.lifecycle",
    {
      lifecycle: "LEADER_REAPED",
      leg,
      legIndex,
      token,
      terminalKind: finalReap.parsedResult.fields[3],
      terminalCode: finalReap.parsedResult.fields[4],
    },
    finalReap,
    deadline,
  );
  for (const capture of state.proofs.values()) {
    for (const row of capture.rows) {
      if (!tombstones.has(row.pid)) {
        tombstones.add(row.pid, "S2_RUBY_GENERATION_RETIRED");
      }
    }
  }
  retireRubyRoots(roots, socketIdentities);
  fifo.retireExternal(batch, deadline, new Set([0]));
  const rootReceipts = Object.freeze(
    Object.fromEntries(
      Object.entries(roots.receipts).map(([role, receipt]) => [
        role,
        Object.freeze({
          pathHash: receipt.pathHash,
          dev: receipt.dev,
          ino: receipt.ino,
        }),
      ]),
    ),
  );
  evidence.add(
    "ruby",
    `ruby.${leg}`,
    {
      legIndex,
      token,
      rootReceipts,
      targetLiteralSha256: sha256(
        Buffer.from(S2_TARGET_LITERAL),
      ),
      supervisorLiteralSha256: sha256(
        Buffer.from(CUSTODY_SUPERVISOR_LITERAL),
      ),
      receipt: captureFacts(receiptTransport),
      receiptRaw: s2RawFact(receiptTransport.body),
      receiptLatch: receiptTransport.latch,
      receiptCounters: receiptTransport.counters,
      receiptRecords: receiptTransport.records,
      output: {
        stdout: captureFacts(stdoutResult),
        stderr: captureFacts(stderrResult),
        stdoutRaw: s2RawFact(stdoutResult.body),
        stderrRaw: s2RawFact(stderrResult.body),
      },
      proofOrdinals: Object.freeze(
        [...state.consumed].sort((left, right) => left - right),
      ),
      transitions: transitionRecords.map(
        (record) => record.hash,
      ),
      finalReapHash: finalReapRecord.hash,
      socketSources: socketIdentities.map(
        (identity) => identity.source,
      ),
    },
    deadline,
  );
  worker.capacity.complete(reservations.leg);
  if (reservations.sockets !== null) {
    worker.capacity.complete(reservations.sockets);
  }
  return Object.freeze({
    rootReceipts,
    token,
    proofOrdinals: Object.freeze(
      [...state.consumed].sort((left, right) => left - right),
    ),
    transitions: transitionRecords.length,
  });
}

async function deliverS2NodeStop({
  supervisor,
  supervisorReader,
  workerStream,
  workerReader,
  workerWriterBudget,
  rawIntent,
  sequence,
  supervisorResult = Buffer.alloc(0),
  readFault = undefined,
  deadline,
}) {
  let resultEof = supervisorReader.ended;
  const secondary = [];
  try {
    supervisor.child.stdin?.end();
  } catch (error) {
    secondary.push(error);
  }
  if (!resultEof && readFault === undefined) {
    try {
      await supervisorReader.requireEof();
      resultEof = true;
    } catch (error) {
      secondary.push(error);
    }
  }
  let supervisorTerminal = supervisor.terminal.current();
  if (supervisorTerminal === null) {
    try {
      const settled = await settleDirectChild(
        supervisor,
        deadline,
        {
          normalMs: 1_000,
          termMs: 500,
          killMs: 500,
          label: "s2-custody-supervisor-node-stop",
          allowSignal: true,
        },
      );
      supervisorTerminal = settled.terminal;
    } catch (error) {
      secondary.push(error);
      supervisorTerminal = supervisor.terminal.current();
    }
  }
  if (supervisorTerminal === null) {
    const custodyFault = new HostAuthorityError(
      "SUPERVISOR_TCB_STOP",
      "NODE_STOP lacks custody-supervisor terminal custody",
      { secondary: secondary.map(safeError) },
    );
    secondary.push(custodyFault);
    supervisorTerminal = Object.freeze({
      code: null,
      error: safeError(custodyFault),
      signal: null,
    });
  }
  const parsedStop =
    supervisorResult.length === 0
      ? null
      : parseS2SupervisorStop(supervisorResult);
  const firstFault =
    parsedStop ??
    Object.freeze({
      kind:
        supervisorTerminal.error === null &&
        supervisorTerminal.code !== null
          ? "SUPERVISOR_DEATH_STOP"
          : "SUPERVISOR_TRANSPORT_STOP",
      faultCode:
        typeof readFault?.code === "string"
          ? readFault.code
              .toUpperCase()
              .replace(/[^A-Z0-9_.-]/gu, "_")
              .slice(0, 80)
          : "SUPERVISOR_RESULT_EOF",
    });
  const readerSnapshot = supervisorReader.faultSnapshot();
  const envelope = Object.freeze({
    schema: 1,
    sequence: sequence + 1,
    kind: "NODE_STOP",
    stopKind: firstFault.kind,
    faultCode: firstFault.faultCode,
    workerIntentSha256: sha256(rawIntent),
    supervisorResult: s2RawFact(supervisorResult),
    readerFault: readerSnapshot.raw,
    resultEof,
    secondaries: s2SecondaryFacts(secondary),
    supervisorTerminal,
  });
  requireS2NodeStopShape(envelope, "Node");
  const rawEnvelope = encodeS2JsonFrame(
    envelope,
    S2_PROTOCOL.resultEnvelopeBytes,
    "Node STOP envelope",
  );
  try {
    await writeS2Frame(
      workerStream,
      rawEnvelope,
      "Node STOP envelope",
      workerWriterBudget,
      deadline,
    );
  } catch (writeFault) {
    throw new HostAuthorityError(
      firstFault.faultCode,
      `custody supervisor selected ${firstFault.kind}`,
      {
        firstFault: Object.freeze({
          code: firstFault.faultCode,
          kind: firstFault.kind,
          supervisorResult: s2RawFact(supervisorResult),
        }),
        secondaryFaults: Object.freeze([
          ...secondary.map(safeError),
          safeError(writeFault),
        ]),
        retained: true,
      },
    );
  }
  workerStream.end();
  let workerRelayEof = true;
  let relayCloseoutFault = null;
  try {
    await workerReader.requireEof();
  } catch (error) {
    workerRelayEof = false;
    relayCloseoutFault = safeError(error);
  }
  return Object.freeze({
    nodeStop: envelope,
    rawNodeStop: rawEnvelope,
    supervisorTerminal,
    secondary: Object.freeze(secondary.map(safeError)),
    workerRelayEof,
    relayCloseoutFault,
  });
}

async function runS2RelayServer({
  supervisor,
  supervisorReader,
  readyRaw,
  workerStream,
  worker,
  deadline,
}) {
  const ready = s2ParseReady(readyRaw);
  const legByToken = new Map(
    S2_LEGS.map((leg, index) => [ready.tokens[index], leg]),
  );
  const workerReader = new S2FrameReader(
    workerStream,
    "Node worker relay intent",
    new S2TransportBudget(
      "Node worker relay intent",
      S2_PROTOCOL.relayFrames,
      S2_PROTOCOL.relayFrameBytes,
    ),
    deadline,
  );
  const workerWriterBudget = new S2TransportBudget(
    "Node worker relay envelope",
    S2_PROTOCOL.relayEnvelopes,
    S2_PROTOCOL.relayEnvelopeBytes,
    S2_PROTOCOL.resultEnvelopeBytes,
  );
  const supervisorWriterBudget = new S2TransportBudget(
    "Node supervisor command",
    S2_PROTOCOL.supervisorInputFrames,
    S2_PROTOCOL.supervisorInputBytes,
  );
  const readyEnvelope = encodeS2JsonFrame(
    Object.freeze({
      schema: 1,
      sequence: 0,
      kind: "READY",
      workerIntentBase64: null,
      supervisorCommandBase64: null,
      supervisorCommandSha256: null,
      supervisorResultBase64: readyRaw.toString("base64"),
      supervisorResultSha256: sha256(readyRaw),
    }),
    S2_PROTOCOL.resultEnvelopeBytes,
    "Node READY envelope",
  );
  await writeS2Frame(
    workerStream,
    readyEnvelope,
    "Node READY envelope",
    workerWriterBudget,
    deadline,
  );
  let previousEnvelopeSha256 = sha256(readyEnvelope);
  let finalAck;
  for (
    let sequence = 0;
    sequence < S2_PROTOCOL.supervisorInputFrames;
    sequence += 1
  ) {
    const rawIntent = await workerReader.read();
    requireCondition(
      rawIntent !== null,
      "host.s2_protocol",
      "worker relay ended before all intents",
      { sequence },
    );
    const intent = parseS2JsonFrame(
      rawIntent,
      "Node worker intent",
    );
    requireS2IntentShape(intent, "Node worker");
    requireCondition(
      intent.schema === 1 &&
        intent.sequence === sequence &&
        intent.kind !== "COMMIT_ACK" &&
        intent.ack?.envelopeSha256 ===
          previousEnvelopeSha256 &&
        Number.isSafeInteger(intent.ack?.evidenceSequence) &&
        typeof intent.ack?.evidenceHash === "string" &&
        /^[a-f0-9]{64}$/u.test(intent.ack.evidenceHash),
      "host.s2_protocol",
      "worker intent sequence/ack differs",
      { sequence, intent },
    );
    const supervisorCommand = s2SupervisorCommand(intent);
    await writeS2Frame(
      supervisor.child.stdin,
      supervisorCommand,
      "Node supervisor command",
      supervisorWriterBudget,
      deadline,
    );
    if (intent.kind === "CLOSE") {
      supervisor.child.stdin.end();
    }
    let supervisorResult;
    try {
      supervisorResult = await supervisorReader.read();
    } catch (error) {
      return deliverS2NodeStop({
        supervisor,
        supervisorReader,
        workerStream,
        workerReader,
        workerWriterBudget,
        rawIntent,
        sequence,
        readFault: error,
        deadline,
      });
    }
    if (supervisorResult === null) {
      return deliverS2NodeStop({
        supervisor,
        supervisorReader,
        workerStream,
        workerReader,
        workerWriterBudget,
        rawIntent,
        sequence,
        readFault: new HostAuthorityError(
          "SUPERVISOR_RESULT_EOF",
          "custody supervisor ended before command result",
          { sequence, kind: intent.kind },
        ),
        deadline,
      });
    }
    const stop = parseS2SupervisorStop(supervisorResult);
    if (stop !== null) {
      return deliverS2NodeStop({
        supervisor,
        supervisorReader,
        workerStream,
        workerReader,
        workerWriterBudget,
        rawIntent,
        sequence,
        supervisorResult,
        deadline,
      });
    }
    const parsedResult =
      parseS2SupervisorFrame(supervisorResult);
    s2ValidateSupervisorResult(
      parsedResult,
      intent,
      sequence + 1,
      intent.token === null
        ? undefined
        : legByToken.get(intent.token),
    );
    const envelope = encodeS2JsonFrame(
      Object.freeze({
        schema: 1,
        sequence: sequence + 1,
        kind: intent.kind,
        workerIntentBase64: rawIntent.toString("base64"),
        supervisorCommandBase64:
          supervisorCommand.toString("base64"),
        supervisorCommandSha256: sha256(supervisorCommand),
        supervisorResultBase64:
          supervisorResult.toString("base64"),
        supervisorResultSha256: sha256(supervisorResult),
      }),
      S2_PROTOCOL.resultEnvelopeBytes,
      "Node result envelope",
    );
    await writeS2Frame(
      workerStream,
      envelope,
      "Node result envelope",
      workerWriterBudget,
      deadline,
    );
    previousEnvelopeSha256 = sha256(envelope);
  }
  const rawAck = await workerReader.read();
  requireCondition(
    rawAck !== null,
    "host.s2_protocol",
    "worker relay ended before final ACK",
  );
  finalAck = parseS2JsonFrame(rawAck, "Node final ACK");
  requireS2FinalAckShape(finalAck, "Node");
  requireCondition(
    finalAck.schema === 1 &&
      finalAck.sequence === 27 &&
      finalAck.kind === "COMMIT_ACK" &&
      Number.isSafeInteger(finalAck.evidenceSequence) &&
      typeof finalAck.evidenceHash === "string" &&
      /^[a-f0-9]{64}$/u.test(finalAck.evidenceHash) &&
      finalAck.envelopeSha256 === previousEnvelopeSha256,
    "host.s2_protocol",
    "worker final ACK relation differs",
    { finalAck },
  );
  await workerReader.requireEof();
  await supervisorReader.requireEof();
  return Object.freeze({
    finalAck,
    rawFinalAck: rawAck,
    previousEnvelopeSha256,
    workerPid: worker.pid,
    supervisorPidPrivateToNode: true,
  });
}

function findEvidenceRecord(parsed, kind) {
  const records = parsed.records.filter((record) => record.kind === kind);
  requireCondition(
    records.length === 1,
    "host.evidence",
    `evidence must contain exactly one ${kind} record`,
    { count: records.length },
  );
  return records[0];
}

function decodeS2RawFact(
  fact,
  label,
  maximum = S2_PROTOCOL.resultEnvelopeBytes,
) {
  requireCondition(
    fact !== null &&
      typeof fact === "object" &&
      Object.keys(fact).sort().join(",") ===
        "base64,bytes,sha256" &&
      typeof fact.base64 === "string" &&
      Number.isSafeInteger(fact.bytes) &&
      fact.bytes >= 0 &&
      fact.bytes <= maximum &&
      typeof fact.sha256 === "string" &&
      /^[a-f0-9]{64}$/u.test(fact.sha256),
    "host.s2_semantic",
    `${label} raw fact shape differs`,
  );
  const bytes = Buffer.from(fact.base64, "base64");
  requireCondition(
    bytes.toString("base64") === fact.base64 &&
      bytes.length === fact.bytes &&
      sha256(bytes) === fact.sha256,
    "host.s2_semantic",
    `${label} raw fact bytes differ`,
  );
  return bytes;
}

function exactKindCounts(records) {
  const counts = {};
  for (const record of records) {
    counts[record.kind] = (counts[record.kind] ?? 0) + 1;
  }
  return Object.freeze(counts);
}

function validateS2RootReceiptFact(receipt, label) {
  requireCondition(
    receipt !== null &&
      typeof receipt === "object" &&
      isCanonicalDecimal(receipt.dev) &&
      isCanonicalDecimal(receipt.ino) &&
      /^[a-f0-9]{64}$/u.test(receipt.pathHash),
    "host.s2_semantic",
    `${label} root receipt differs`,
    { receipt },
  );
}

function expectedS2RootCreationPlan(invocation) {
  const preflight = join(invocation, "preflight");
  const fifoRoot = join(preflight, "fifo");
  const rubyRoot = join(preflight, "ruby");
  const plan = [
    ["invocation", invocation],
    ["evidence-root", join(invocation, "evidence")],
    ["preflight", preflight],
    ["fifo-root", fifoRoot],
    ["node-support", join(preflight, "node-stream")],
    ["fifo-b040", join(fifoRoot, "b040")],
    ["node-denial", join(preflight, "node-stream")],
    ["fifo-b041", join(fifoRoot, "b041")],
    ["ruby-container", rubyRoot],
  ];
  let proofBatch = 0;
  for (const [legIndex, leg] of S2_LEGS.entries()) {
    for (const role of ["home", "tmp", "cwd", "parent", "child"]) {
      plan.push([`ruby-${role}`, join(rubyRoot, role)]);
    }
    plan.push([
      `fifo-b${String(36 + legIndex).padStart(3, "0")}`,
      join(
        fifoRoot,
        `b${String(36 + legIndex).padStart(3, "0")}`,
      ),
    ]);
    for (let index = 0; index < S2_PROOF_ORDINALS[leg].length; index += 1) {
      plan.push([
        `fifo-b${String(proofBatch).padStart(3, "0")}`,
        join(
          fifoRoot,
          `b${String(proofBatch).padStart(3, "0")}`,
        ),
      ]);
      proofBatch += 1;
    }
  }
  requireCondition(
    plan.length === 69 && proofBatch === 36,
    "host.s2_semantic",
    "derived root creation plan cardinality differs",
    { roots: plan.length, proofBatch },
  );
  return Object.freeze(
    plan.map(([role, path]) =>
      Object.freeze({
        role,
        pathHash: sha256(Buffer.from(path)),
      })
    ),
  );
}

function expectedS2FifoBatchOrder() {
  const order = [40, 41];
  let proofBatch = 0;
  for (const [legIndex, leg] of S2_LEGS.entries()) {
    order.push(36 + legIndex);
    for (
      let index = 0;
      index < S2_PROOF_ORDINALS[leg].length;
      index += 1
    ) {
      order.push(proofBatch);
      proofBatch += 1;
    }
  }
  requireCondition(
    order.length === 42 &&
      proofBatch === 36 &&
      new Set(order).size === order.length,
    "host.s2_semantic",
    "derived FIFO creation order differs",
    { order, proofBatch },
  );
  return Object.freeze(order);
}

function validateS2RootInventory(inventory, invocation) {
  requireCondition(
    Array.isArray(inventory) && inventory.length === 69,
    "host.s2_semantic",
    "worker root inventory cardinality differs",
    { roots: inventory?.length },
  );
  const expected = expectedS2RootCreationPlan(invocation);
  inventory.forEach((receipt, index) => {
    requireExactKeys(
      receipt,
      [
        "dev",
        "gid",
        "ino",
        "mode",
        "nlink",
        "pathHash",
        "role",
        "type",
        "uid",
      ],
      "host.s2_semantic",
      `root inventory receipt ${index}`,
    );
    requireCondition(
      receipt.role === expected[index].role &&
        receipt.pathHash === expected[index].pathHash &&
        receipt.type === "directory" &&
        receipt.mode === 0o700 &&
        receipt.uid === HOST_UID &&
        receipt.gid === HOST_GID &&
        Number.isInteger(receipt.nlink) &&
        receipt.nlink >= 2 &&
        isCanonicalDecimal(receipt.dev) &&
        isCanonicalDecimal(receipt.ino),
      "host.s2_semantic",
      "worker root inventory receipt differs",
      { index, receipt, expected: expected[index] },
    );
  });
  const recreatedPairs = [[4, 6]];
  for (let roleOffset = 0; roleOffset < 5; roleOffset += 1) {
    for (const [leftBase, rightBase] of [
      [9, 20],
      [20, 31],
      [31, 50],
    ]) {
      recreatedPairs.push([
        leftBase + roleOffset,
        rightBase + roleOffset,
      ]);
    }
  }
  for (const [left, right] of recreatedPairs) {
    requireCondition(
      inventory[left].pathHash === inventory[right].pathHash &&
        (inventory[left].dev !== inventory[right].dev ||
          inventory[left].ino !== inventory[right].ino),
      "host.s2_semantic",
      "recreated root reused its prior object identity",
      { left, right },
    );
  }
  return expected;
}

function validateS2CaptureRawFacts(
  facts,
  label,
  expectedBody,
  maximum,
) {
  const stdout = decodeS2RawFact(
    facts.stdoutRaw,
    `${label} stdout`,
    maximum,
  );
  const stderr = decodeS2RawFact(
    facts.stderrRaw,
    `${label} stderr`,
    maximum,
  );
  requireCondition(
    stdout.equals(expectedBody) &&
      stderr.length === 0 &&
      facts.stdout.bytes === stdout.length &&
      facts.stdout.sha256 === sha256(stdout) &&
      facts.stdout.eof === true &&
      facts.stderr.bytes === 0 &&
      facts.stderr.sha256 === sha256(stderr) &&
      facts.stderr.eof === true,
    "host.s2_semantic",
    `${label} raw output projection differs`,
  );
  return Object.freeze({ stdout, stderr });
}

function validateS2TombstoneFacts(facts) {
  requireExactKeys(
    facts,
    [
      "bitsetBase64",
      "bytes",
      "count",
      "records",
      "recordsSha256",
      "sha256",
    ],
    "host.s2_semantic",
    "tombstone facts",
  );
  const bitset = Buffer.from(facts.bitsetBase64, "base64");
  requireCondition(
    bitset.toString("base64") === facts.bitsetBase64 &&
      bitset.length === TOMBSTONE_BYTES &&
      facts.bytes === TOMBSTONE_BYTES &&
      sha256(bitset) === facts.sha256 &&
      Array.isArray(facts.records) &&
      facts.records.length === facts.count &&
      sha256(Buffer.from(canonicalJson(facts.records))) ===
        facts.recordsSha256,
    "host.s2_semantic",
    "tombstone bytes or records differ",
  );
  const seen = new Set();
  for (const record of facts.records) {
    requireExactKeys(
      record,
      ["pid", "reason"],
      "host.s2_semantic",
      "tombstone record",
    );
    requireCondition(
      Number.isInteger(record.pid) &&
        record.pid >= PID_MIN &&
        record.pid <= PID_MAX &&
        !seen.has(record.pid) &&
        /^[A-Z0-9_]{1,80}$/u.test(record.reason),
      "host.s2_semantic",
      "tombstone record is invalid or duplicate",
      { record },
    );
    seen.add(record.pid);
    const byte = Math.floor(record.pid / 8);
    const bit = record.pid % 8;
    requireCondition(
      (bitset[byte] & 2 ** bit) !== 0,
      "host.s2_semantic",
      "tombstone record lacks its bit",
      { record },
    );
  }
  let setBits = 0;
  for (const byte of bitset) {
    let value = byte;
    while (value !== 0) {
      setBits += value & 1;
      value >>= 1;
    }
  }
  requireCondition(
    setBits === facts.count,
    "host.s2_semantic",
    "tombstone bitset contains an unreported PID",
    { setBits, count: facts.count },
  );
}

function validateS2Evidence(parsed, invocation) {
  requireCondition(
    parsed.records.length ===
      S2_PROTOCOL.cleanEvidenceRecords,
    "host.s2_semantic",
    "semantic checker requires the exact clean record cardinality",
    { records: parsed.records.length },
  );
  const expectedKinds = Object.freeze({
    "supervisor.initial_static": 1,
    "supervisor.setup": 1,
    "supervisor.pre_spawn": 1,
    "worker.entry": 1,
    "supervisor.lifecycle": 10,
    "worker.preflight": 1,
    "fifo.batch": 42,
    "node.support": 1,
    "node.denial": 1,
    "ruby.proof": 36,
    "supervisor.transition": 18,
    "ruby.support": 1,
    "ruby.denial": 1,
    "ruby.one-receipt": 1,
    "ruby.parent-loss": 1,
    "fifo.lifecycle": 1,
    "worker.tombstone": 1,
    "worker.model": 1,
    "worker.final": 1,
  });
  requireCondition(
    canonicalJson(exactKindCounts(parsed.records)) ===
      canonicalJson(expectedKinds),
    "host.s2_semantic",
    "semantic checker record-kind inventory differs",
    {
      actual: exactKindCounts(parsed.records),
      expected: expectedKinds,
    },
  );
  const initialRecord = findEvidenceRecord(
    parsed,
    "supervisor.initial_static",
  );
  const setupRecord = findEvidenceRecord(
    parsed,
    "supervisor.setup",
  );
  const preSpawnRecord = findEvidenceRecord(
    parsed,
    "supervisor.pre_spawn",
  );
  const workerEntryRecord = findEvidenceRecord(
    parsed,
    "worker.entry",
  );
  const preflightRecord = findEvidenceRecord(
    parsed,
    "worker.preflight",
  );
  const initialStatic = validateStaticSnapshot(
    initialRecord.facts.static,
  );
  requireCondition(
    isCanonicalDecimal(initialRecord.facts.startNs) &&
      isCanonicalDecimal(
        initialRecord.facts.outerDeadlineNs,
      ) &&
      BigInt(initialRecord.facts.outerDeadlineNs) -
        BigInt(initialRecord.facts.startNs) ===
        660_000_000_000n,
    "host.s2_semantic",
    "initial static deadline relation differs",
  );
  requireExactKeys(
    setupRecord.facts,
    ["evidence", "invocation", "packet", "runToken", "version"],
    "host.s2_semantic",
    "supervisor setup",
  );
  validateS2RootReceiptFact(
    setupRecord.facts.invocation,
    "supervisor invocation",
  );
  requireExactKeys(
    setupRecord.facts.evidence,
    ["dev", "ino", "nlink", "pathHash"],
    "host.s2_semantic",
    "supervisor evidence file",
  );
  requireCondition(
    setupRecord.facts.version === S2_PACKET.version &&
      canonicalJson(setupRecord.facts.packet) ===
        canonicalJson(S2_PACKET) &&
      /^[a-f0-9]{64}$/u.test(setupRecord.facts.runToken) &&
      setupRecord.facts.runToken.slice(0, 8) ===
        basename(invocation.retainedInvocation)
          .slice("marrow-vsq-a-".length, -7) &&
      setupRecord.facts.invocation.pathHash ===
        sha256(Buffer.from(invocation.retainedInvocation)) &&
      setupRecord.facts.invocation.dev ===
        invocation.invocationReceipt.dev &&
      setupRecord.facts.invocation.ino ===
        invocation.invocationReceipt.ino &&
      setupRecord.facts.evidence.dev ===
        invocation.evidenceIdentity.dev &&
      setupRecord.facts.evidence.ino ===
        invocation.evidenceIdentity.ino &&
      setupRecord.facts.evidence.nlink === 1 &&
      setupRecord.facts.evidence.nlink ===
        invocation.evidenceIdentity.nlink &&
      setupRecord.facts.evidence.pathHash ===
        sha256(
          Buffer.from(
            join(
              invocation.retainedInvocation,
              "evidence",
              "a0.jsonl",
            ),
          ),
        ),
    "host.s2_semantic",
    "supervisor setup/root relation differs",
  );
  requireCondition(
    canonicalJson(preSpawnRecord.facts.node) ===
        canonicalJson(initialStatic.files[0]) &&
      canonicalJson(preSpawnRecord.facts.owner) ===
        canonicalJson(initialStatic.owner) &&
      /^[a-f0-9]{64}$/u.test(
        preSpawnRecord.facts.basisToken,
      ) &&
      isCanonicalDecimal(
        preSpawnRecord.facts.preSpawnNowNs,
      ) &&
      preSpawnRecord.facts.outerDeadlineNs ===
        initialRecord.facts.outerDeadlineNs &&
      isCanonicalDecimal(
        preSpawnRecord.facts.workerDeadlineNs,
      ) &&
      isCanonicalDecimal(
        preSpawnRecord.facts.workerRemainingNs,
      ) &&
      BigInt(preSpawnRecord.facts.workerDeadlineNs) -
        BigInt(preSpawnRecord.facts.preSpawnNowNs) ===
        BigInt(preSpawnRecord.facts.workerRemainingNs) &&
      BigInt(preSpawnRecord.facts.workerDeadlineNs) <=
        BigInt(preSpawnRecord.facts.outerDeadlineNs) &&
      canonicalJson(preSpawnRecord.facts.protocol) ===
        canonicalJson(S2_PROTOCOL) &&
      canonicalJson(preSpawnRecord.facts.packet) ===
        canonicalJson(S2_PACKET),
    "host.s2_semantic",
    "pre-spawn static/deadline relation differs",
  );
  requireCondition(
    workerEntryRecord.facts.basisToken ===
        preSpawnRecord.facts.basisToken &&
      workerEntryRecord.facts.initialChain ===
        preSpawnRecord.hash &&
      canonicalJson(workerEntryRecord.facts.owner) ===
        canonicalJson(initialStatic.owner) &&
      Number.isInteger(workerEntryRecord.facts.pid) &&
      Number.isInteger(workerEntryRecord.facts.ppid) &&
      isCanonicalDecimal(workerEntryRecord.facts.entryNowNs) &&
      isCanonicalDecimal(
        workerEntryRecord.facts.entryRemainingNs,
      ) &&
      BigInt(workerEntryRecord.facts.entryNowNs) <=
        BigInt(preSpawnRecord.facts.workerDeadlineNs),
    "host.s2_semantic",
    "worker entry authority relation differs",
  );
  for (const [label, receipt] of Object.entries(
    preflightRecord.facts,
  )) {
    validateS2RootReceiptFact(receipt, `worker ${label}`);
  }
  requireCondition(
    preflightRecord.facts.canary.sha256 ===
      sha256(CANARY_BYTES),
    "host.s2_semantic",
    "preflight canary digest differs",
  );
  const nodeRecords = [
    findEvidenceRecord(parsed, "node.support"),
    findEvidenceRecord(parsed, "node.denial"),
  ];
  nodeRecords.forEach((record, index) => {
    const support = index === 0;
    const label = support ? "node support" : "node denial";
    const stdout = decodeS2RawFact(
      record.facts.stdoutRaw,
      `${label} stdout`,
      256,
    );
    const stderr = decodeS2RawFact(
      record.facts.stderrRaw,
      `${label} stderr`,
      256,
    );
    const output = nodeOutput(stdout, support);
    const rootPath = join(
      invocation.retainedInvocation,
      "preflight",
      "node-stream",
    );
    const socketPath = join(rootPath, "control.sock");
    const profile = sandboxProfile(
      support ? "node-support" : "node-denial",
      support ? [socketPath] : [],
    );
    const argvProjection = Object.freeze([
      "-p",
      profile.sha256,
      NODE,
      "-e",
      sha256(Buffer.from(NODE_LITERAL)),
      "--",
      sha256(Buffer.from(socketPath)),
    ]);
    requireCondition(
      record.kind ===
          (support ? "node.support" : "node.denial") &&
        record.facts.literalSha256 ===
          sha256(Buffer.from(NODE_LITERAL)) &&
        record.facts.profileSha256 === profile.sha256 &&
        decodeS2RawFact(
          record.facts.profileRaw,
          `${label} profile`,
          4_096,
        ).equals(profile.bytes) &&
        record.facts.socketPathHash ===
          sha256(Buffer.from(socketPath)) &&
        record.facts.rootPathHash ===
          sha256(Buffer.from(rootPath)) &&
        canonicalJson(record.facts.argvProjection) ===
          canonicalJson(argvProjection) &&
        record.facts.argvSha256 ===
          sha256(Buffer.from(canonicalJson(argvProjection))) &&
        record.facts.root.pathHash ===
          sha256(Buffer.from(rootPath)) &&
        isCanonicalDecimal(record.facts.root.dev) &&
        isCanonicalDecimal(record.facts.root.ino) &&
        record.facts.terminal.error === null &&
        record.facts.terminal.code === 0 &&
        record.facts.terminal.signal === null &&
        stderr.length === 0 &&
        record.facts.stdout.bytes === stdout.length &&
        record.facts.stdout.sha256 === sha256(stdout) &&
        record.facts.stdout.eof === true &&
        record.facts.stderr.bytes === 0 &&
        record.facts.stderr.sha256 === sha256(stderr) &&
        record.facts.stderr.eof === true &&
        canonicalJson(record.facts.output) ===
          canonicalJson(output) &&
        output.ppid === workerEntryRecord.facts.pid,
      "host.s2_semantic",
      `${label} static/output/root relation differs`,
    );
  });
  requireCondition(
    nodeRecords[0].facts.root.dev !==
        nodeRecords[1].facts.root.dev ||
      nodeRecords[0].facts.root.ino !==
        nodeRecords[1].facts.root.ino,
    "host.s2_semantic",
    "Node support/denial root inode was reused",
  );
  const fifoIndices = parsed.records
    .filter((record) => record.kind === "fifo.batch")
    .map((record) => record.facts.index)
    .sort((left, right) => left - right);
  const fifoCreationOrder = parsed.records
    .filter((record) => record.kind === "fifo.batch")
    .map((record) => record.facts.index);
  requireCondition(
    canonicalJson(fifoIndices) ===
      canonicalJson(
        Array.from({ length: 42 }, (_, index) => index),
      ) &&
      canonicalJson(fifoCreationOrder) ===
        canonicalJson(expectedS2FifoBatchOrder()),
    "host.s2_semantic",
    "semantic checker FIFO ordinal/order inventory differs",
    { fifoIndices, fifoCreationOrder },
  );
  const fifoRecords = parsed.records.filter(
    (record) => record.kind === "fifo.batch",
  );
  let derivedFifoPathBytes = 0;
  const derivedFifoFacts = [];
  for (const record of fifoRecords) {
    const index = record.facts.index;
    const names =
      index >= 36 && index <= 39
        ? ["receipt.fifo", "stdout.fifo", "stderr.fifo"]
        : ["stdout.fifo", "stderr.fifo"];
    const batchPath = join(
      invocation.retainedInvocation,
      "preflight",
      "fifo",
      `b${String(index).padStart(3, "0")}`,
    );
    const paths = names.map((name) => join(batchPath, name));
    const pathBytes = paths.map((path) =>
      Buffer.byteLength(path),
    );
    const argvBytes = [MKFIFO, "-m", "600", ...paths].reduce(
      (total, value) =>
        total + Buffer.byteLength(value) + 1,
      0,
    );
    requireCondition(
      canonicalJson(record.facts.paths) ===
        canonicalJson(
          paths.map((path) => sha256(Buffer.from(path))),
        ) &&
        canonicalJson(record.facts.pathBytes) ===
          canonicalJson(pathBytes) &&
        record.facts.batchPathHash ===
          sha256(Buffer.from(batchPath)) &&
        record.facts.argvBytes === argvBytes &&
        record.facts.terminal.error === null &&
        record.facts.terminal.code === 0 &&
        record.facts.terminal.signal === null &&
        record.facts.externalWriter ===
          (index >= 36 && index <= 39 ? true : undefined),
      "host.s2_semantic",
      "semantic checker FIFO batch projection differs",
      { index },
    );
    derivedFifoPathBytes += pathBytes.reduce(
      (total, value) => total + value,
      0,
    );
    record.facts.paths.forEach((pathHash) => {
      derivedFifoFacts.push([index, pathHash]);
    });
  }
  const fifoLifecycle = findEvidenceRecord(
    parsed,
    "fifo.lifecycle",
  );
  requireExactKeys(
    fifoLifecycle.facts,
    ["count", "digest", "facts"],
    "host.s2_semantic",
    "FIFO lifecycle facts",
  );
  requireCondition(
    fifoLifecycle.facts.count === 88 &&
      Array.isArray(fifoLifecycle.facts.facts) &&
      fifoLifecycle.facts.facts.length === 88 &&
      derivedFifoFacts.length === 88 &&
      derivedFifoPathBytes === 6_428 &&
      sha256(
        Buffer.from(canonicalJson(fifoLifecycle.facts.facts)),
      ) === fifoLifecycle.facts.digest,
    "host.s2_semantic",
    "semantic checker FIFO lifecycle cardinality/digest differs",
  );
  const fifoObjectIdentities = new Set();
  fifoLifecycle.facts.facts.forEach((fact, index) => {
    requireCondition(
      Array.isArray(fact) &&
        fact.length === 4 &&
        fact[0] === derivedFifoFacts[index][0] &&
        fact[1] === derivedFifoFacts[index][1] &&
        isCanonicalDecimal(fact[2]) &&
        isCanonicalDecimal(fact[3]) &&
        !fifoObjectIdentities.has(`${fact[2]}:${fact[3]}`),
      "host.s2_semantic",
      "semantic checker FIFO lifecycle receipt differs",
      { index, fact, derived: derivedFifoFacts[index] },
    );
    fifoObjectIdentities.add(`${fact[2]}:${fact[3]}`);
  });
  const proofRecords = parsed.records.filter(
    (record) => record.kind === "ruby.proof",
  );
  const receiptTopologies = new Map();
  const expectedProofs = S2_LEGS.flatMap((leg) =>
    S2_PROOF_ORDINALS[leg].map((ordinal) => [leg, ordinal])
  );
  requireCondition(
    canonicalJson(
      proofRecords.map((record) => [
        record.facts.leg,
        record.facts.ordinal,
      ]),
    ) === canonicalJson(expectedProofs),
    "host.s2_semantic",
    "semantic checker proof order differs",
  );
  const capturePids = new Set();
  proofRecords.forEach((record, index) => {
    const facts = record.facts;
    requireExactKeys(
      facts,
      [
        "attempt",
        "attemptStates",
        "capturePid",
        "leg",
        "ordinal",
        "pgidObserved",
        "rows",
        "stderr",
        "stdout",
        "terminal",
      ],
      "host.s2_semantic",
      `Ruby proof ${index}`,
    );
    requireExactKeys(
      facts.attempt,
      ["batchIndex", "ordinal", "replacement"],
      "host.s2_semantic",
      `Ruby proof attempt ${index}`,
    );
    requireExactKeys(
      facts.stdout,
      ["base64", "bytes", "eof", "sha256"],
      "host.s2_semantic",
      `Ruby proof stdout ${index}`,
    );
    requireExactKeys(
      facts.stderr,
      ["base64", "bytes", "eof", "sha256"],
      "host.s2_semantic",
      `Ruby proof stderr ${index}`,
    );
    requireExactKeys(
      facts.terminal,
      ["code", "error", "signal"],
      "host.s2_semantic",
      `Ruby proof terminal ${index}`,
    );
    requireCondition(
      Number.isInteger(facts.capturePid) &&
        facts.capturePid >= PID_MIN &&
        facts.capturePid <= PID_MAX &&
        !capturePids.has(facts.capturePid) &&
        facts.attempt.ordinal === index + 1 &&
        facts.attempt.batchIndex === index &&
        facts.attempt.replacement === false &&
        canonicalJson(facts.attemptStates) ===
          canonicalJson([
            "Reserved",
            "AttemptMaterialized",
            "CaptureLaunched",
            "ProofInstalled",
            "Retired",
            "EvidenceCommitted",
          ]) &&
        facts.stdout.eof === true &&
        facts.stderr.eof === true &&
        facts.stderr.bytes === 0 &&
        facts.stderr.base64 === "" &&
        facts.terminal.code === 0 &&
        facts.terminal.signal === null &&
        facts.terminal.error === null,
      "host.s2_semantic",
      "semantic checker proof attempt/terminal state differs",
      { index, facts },
    );
    capturePids.add(facts.capturePid);
    const stdout = Buffer.from(facts.stdout.base64, "base64");
    const stderr = Buffer.from(facts.stderr.base64, "base64");
    requireCondition(
      stdout.toString("base64") === facts.stdout.base64 &&
        stdout.length === facts.stdout.bytes &&
        stdout.length <=
          (facts.ordinal === 13 || facts.ordinal === 14
            ? 192
            : 384) &&
        sha256(stdout) === facts.stdout.sha256 &&
        stderr.toString("base64") === facts.stderr.base64 &&
        stderr.length === facts.stderr.bytes &&
        sha256(stderr) === facts.stderr.sha256 &&
        canonicalJson(parsePsRows(stdout)) ===
          canonicalJson(facts.rows),
      "host.s2_semantic",
      "semantic checker proof raw-byte projection differs",
      { index },
    );
  });
  const rubyRecordsByLeg = new Map();
  for (const leg of S2_LEGS) {
    const record = findEvidenceRecord(
      parsed,
      `ruby.${leg}`,
    );
    rubyRecordsByLeg.set(leg, record);
    const intentionalOneReceipt = leg === "one-receipt";
    const raw = decodeS2RawFact(
      record.facts.receiptRaw,
      `Ruby ${leg} receipt`,
      384,
    );
    requireCondition(
      raw.length === record.facts.receipt.bytes &&
        sha256(raw) === record.facts.receipt.sha256 &&
        record.facts.receipt.eof === !intentionalOneReceipt,
      "host.s2_semantic",
      `semantic checker Ruby ${leg} receipt fact differs`,
    );
    const lines =
      raw.length === 0
        ? []
        : raw.toString("ascii").slice(0, -1).split("\n");
    const receipts = lines.map((line) =>
      parseRubyReceiptLine(`${line}\n`)
    );
    requireCondition(
      canonicalJson(receipts) ===
        canonicalJson(record.facts.receiptRecords) &&
        receipts.length ===
          (intentionalOneReceipt ? 1 : 2),
      "host.s2_semantic",
      `semantic checker Ruby ${leg} receipt projection differs`,
    );
    receiptTopologies.set(
      leg,
      validateS2ReceiptTopology(
        receipts,
        leg === "denial" ? "D" : "S",
      ),
    );
    const expectedPrefix = intentionalOneReceipt
      ? receipts[0].role
      : `${receipts[0].role}_${receipts[1].role}`;
    const expectedSelectedLeg = intentionalOneReceipt
      ? `ONE_RECEIPT_${receipts[0].role}_FIRST`
      : leg.replace("-", "_").toUpperCase();
    const latch = record.facts.receiptLatch;
    requireCondition(
      latch.authorityPrefix === expectedPrefix &&
        latch.observedPrefix === expectedPrefix &&
        latch.selectedLeg === expectedSelectedLeg &&
        latch.terminal ===
          (intentionalOneReceipt
            ? "PROTOCOL_INTENTIONAL_ONE_RECORD_STOP"
            : "EOF_TERMINAL") &&
        latch.family ===
          (intentionalOneReceipt ? "PROTOCOL" : "EOF") &&
        latch.classifiedRoute ===
          (intentionalOneReceipt
            ? "PARSER_INTENTIONALLY_STOPPED_AFTER_ONE_RECORD"
            : "ORDINARY_SUCCESS") &&
        latch.proofRoute ===
          (intentionalOneReceipt
            ? "EXISTING_COMPLEX_ONE_RECEIPT_BRANCH"
            : leg.replace("-", "_")) &&
        latch.outcome ===
          (intentionalOneReceipt
            ? "ONE_RECEIPT_ELIGIBLE_AFTER_CLEANUP"
            : "ORDINARY_SUCCESS") &&
        latch.trailingLength ===
          (intentionalOneReceipt ? raw.length : 0) &&
        latch.trailingSha256 ===
          (intentionalOneReceipt ? sha256(raw) : "NONE"),
      "host.s2_semantic",
      `semantic checker Ruby ${leg} terminal latch differs`,
    );
    const counters = record.facts.receiptCounters;
    requireCondition(
      counters.committedBytes === raw.length &&
        counters.reads === counters.reservationAttempts &&
        counters.reservationReleases +
          counters.committedBytes ===
          counters.reservationAttempts &&
        counters.yields +
          (record.facts.receipt.eof ? 1 : 0) ===
          counters.reservationReleases,
      "host.s2_semantic",
      `semantic checker Ruby ${leg} receipt counters differ`,
    );
    const expectedStdout =
      leg === "support"
        ? Buffer.from("SUCCESS\n")
        : leg === "denial"
          ? Buffer.from("DENIED\n")
          : Buffer.alloc(0);
    validateS2CaptureRawFacts(
      record.facts.output,
      `Ruby ${leg}`,
      expectedStdout,
      4_096,
    );
    const expectedRootHashes = Object.fromEntries(
      ["home", "tmp", "cwd", "parent", "child"].map(
        (role) => [
          role,
          sha256(
            Buffer.from(
              join(
                invocation.retainedInvocation,
                "preflight",
                "ruby",
                role,
              ),
            ),
          ),
        ],
      ),
    );
    requireCondition(
      record.facts.legIndex === S2_LEGS.indexOf(leg),
      "host.s2_semantic",
      `semantic checker Ruby ${leg} leg index differs`,
    );
    requireCondition(
      record.facts.targetLiteralSha256 ===
        sha256(Buffer.from(S2_TARGET_LITERAL)) &&
        record.facts.supervisorLiteralSha256 ===
          sha256(Buffer.from(CUSTODY_SUPERVISOR_LITERAL)) &&
        canonicalJson(record.facts.socketSources) ===
          canonicalJson(
            leg === "denial"
              ? []
              : leg === "one-receipt"
                ? ["receipt", "topology-bound"]
                : ["receipt", "receipt"],
          ) &&
        record.facts.rootReceipts !== null &&
        typeof record.facts.rootReceipts === "object" &&
        Object.keys(record.facts.rootReceipts)
          .sort()
          .join(",") === "child,cwd,home,parent,tmp" &&
        Object.entries(record.facts.rootReceipts).every(
          ([role, receipt]) => {
            validateS2RootReceiptFact(
              receipt,
              `Ruby ${leg} ${role}`,
            );
            return receipt.pathHash === expectedRootHashes[role];
          },
        ),
      "host.s2_semantic",
      `semantic checker Ruby ${leg} literal/root relation differs`,
    );
  }
  for (const leg of S2_LEGS) {
    const topology = receiptTopologies.get(leg);
    const semanticState = {
      leg,
      pgid: topology.pgid,
      topology,
      incarnation: undefined,
    };
    for (const record of proofRecords.filter(
      (candidate) => candidate.facts.leg === leg,
    )) {
      requireCondition(
        record.facts.pgidObserved === topology.pgid,
        "host.s2_semantic",
        `semantic checker Ruby ${leg} observed PGID differs`,
      );
      const incarnation = s2ValidateProductionRows(
        semanticState,
        record.facts.rows,
        record.facts.ordinal,
      );
      if (semanticState.incarnation === undefined) {
        semanticState.incarnation = incarnation;
      }
    }
  }
  const expectedTombstoneRecords = [];
  const expectedTombstonePids = new Set();
  const expectTombstone = (pid, reason) => {
    requireCondition(
      Number.isInteger(pid) &&
        pid >= PID_MIN &&
        pid <= PID_MAX &&
        !expectedTombstonePids.has(pid),
      "host.s2_semantic",
      "derived tombstone PID is invalid or repeated",
      { pid, reason },
    );
    expectedTombstonePids.add(pid);
    expectedTombstoneRecords.push(
      Object.freeze({ pid, reason }),
    );
  };
  for (const record of parsed.records) {
    if (record.kind === "fifo.batch") {
      expectTombstone(
        record.facts.pid,
        "FIFO_DIRECT_REAP",
      );
    } else if (
      record.kind === "node.support" ||
      record.kind === "node.denial"
    ) {
      expectTombstone(
        record.facts.output.pid,
        record.kind === "node.support"
          ? "NODE_SUPPORT_REAP"
          : "NODE_DENIAL_REAP",
      );
    } else if (record.kind === "ruby.proof") {
      expectTombstone(
        record.facts.capturePid,
        "S2_PS_DIRECT_REAP",
      );
    } else if (
      S2_LEGS.some((leg) => record.kind === `ruby.${leg}`)
    ) {
      const leg = record.kind.slice("ruby.".length);
      for (const proofRecord of proofRecords.filter(
        (candidate) => candidate.facts.leg === leg,
      )) {
        for (const row of proofRecord.facts.rows) {
          if (!expectedTombstonePids.has(row.pid)) {
            expectTombstone(
              row.pid,
              "S2_RUBY_GENERATION_RETIRED",
            );
          }
        }
      }
    }
  }
  const tombstoneRecord = findEvidenceRecord(
    parsed,
    "worker.tombstone",
  );
  validateS2TombstoneFacts(tombstoneRecord.facts);
  requireCondition(
    canonicalJson(tombstoneRecord.facts.records) ===
      canonicalJson(expectedTombstoneRecords),
    "host.s2_semantic",
    "worker tombstone order/provenance differs",
    {
      actual: tombstoneRecord.facts.records,
      expected: expectedTombstoneRecords,
    },
  );
  const workerModel = findEvidenceRecord(
    parsed,
    "worker.model",
  );
  requireExactKeys(
    workerModel.facts,
    [
      "actual",
      "capacity",
      "descriptorPeak",
      "finalStaticDigest",
      "legResults",
      "protocol",
      "rootInventory",
      "taskFifoPeak",
      "tombstone",
      "version",
    ],
    "host.s2_semantic",
    "worker model",
  );
  const rootPlan = validateS2RootInventory(
    workerModel.facts.rootInventory,
    invocation.retainedInvocation,
  );
  requireCondition(
    workerModel.facts.rootInventory[0].dev ===
        invocation.invocationReceipt.dev &&
      workerModel.facts.rootInventory[0].ino ===
        invocation.invocationReceipt.ino &&
      workerModel.facts.rootInventory[1].dev ===
        invocation.evidenceRootReceipt.dev &&
      workerModel.facts.rootInventory[1].ino ===
        invocation.evidenceRootReceipt.ino &&
      rootPlan[0].pathHash ===
        invocation.invocationReceipt.pathHash &&
      rootPlan[1].pathHash ===
        invocation.evidenceRootReceipt.pathHash,
    "host.s2_semantic",
    "worker root inventory retained-root relation differs",
  );
  const expectedActual = Object.freeze({
    directories: 69,
    regularFiles: 2,
    fifoBatches: 42,
    fifoInodes: 88,
    fifoPathBytes: derivedFifoPathBytes,
    captureAttempts: proofRecords.length,
    psCaptures: proofRecords.length,
    proofs: proofRecords.length,
    nodeLegs: nodeRecords.length,
    rubyLegs: rubyRecordsByLeg.size,
    sockets: 7,
    rubyCustodySupervisors: 1,
    protocolSocketpairs: 3,
    protocolEndpoints: 6,
    startupPipes: S2_LEGS.length * 2,
    startupPipeEndpoints: S2_LEGS.length,
    descriptorSlots: 59,
  });
  requireExactKeys(
    workerModel.facts.capacity,
    ["completed", "maximum", "reserved"],
    "host.s2_semantic",
    "worker capacity",
  );
  requireCondition(
    canonicalJson(workerModel.facts.actual) ===
        canonicalJson(expectedActual) &&
      canonicalJson(workerModel.facts.capacity.maximum) ===
        canonicalJson(CAPACITY_MAXIMA) &&
      canonicalJson(workerModel.facts.capacity.reserved) ===
        canonicalJson(expectedActual) &&
      canonicalJson(workerModel.facts.capacity.completed) ===
        canonicalJson(expectedActual) &&
      workerModel.facts.version === S2_PACKET.version &&
      workerModel.facts.descriptorPeak === 59 &&
      workerModel.facts.taskFifoPeak === 17 &&
      canonicalJson(workerModel.facts.protocol) ===
        canonicalJson(S2_PROTOCOL) &&
      canonicalJson(workerModel.facts.tombstone) ===
        canonicalJson(tombstoneRecord.facts) &&
      workerModel.facts.finalStaticDigest ===
        initialStatic.digest,
    "host.s2_semantic",
    "worker model is not independently derived",
    { actual: workerModel.facts.actual, expectedActual },
  );
  requireCondition(
    Array.isArray(workerModel.facts.legResults) &&
      workerModel.facts.legResults.length === S2_LEGS.length,
    "host.s2_semantic",
    "worker model leg result cardinality differs",
  );
  workerModel.facts.legResults.forEach((result, index) => {
    const leg = S2_LEGS[index];
    const rubyRecord = rubyRecordsByLeg.get(leg);
    requireExactKeys(
      result,
      ["proofOrdinals", "rootReceipts", "token", "transitions"],
      "host.s2_semantic",
      `worker leg result ${leg}`,
    );
    requireCondition(
      result.token === rubyRecord.facts.token &&
        canonicalJson(result.proofOrdinals) ===
          canonicalJson(S2_PROOF_ORDINALS[leg]) &&
        result.transitions ===
          S2_TRANSITION_PLAN[leg].length + 2 &&
        canonicalJson(result.rootReceipts) ===
          canonicalJson(rubyRecord.facts.rootReceipts),
      "host.s2_semantic",
      `worker model leg result ${leg} differs`,
    );
  });
  const lifecycle = parsed.records.filter(
    (record) => record.kind === "supervisor.lifecycle",
  );
  const readyRecord = lifecycle[0];
  requireCondition(
    readyRecord.facts.lifecycle === "READY",
    "host.s2_semantic",
    "semantic checker lifecycle does not begin at READY",
  );
  const readyResult = decodeS2RawFact(
    readyRecord.facts.supervisorResult,
    "READY supervisor result",
    S2_PROTOCOL.frameBytes,
  );
  const readyEnvelope = decodeS2RawFact(
    readyRecord.facts.nodeEnvelope,
    "READY Node envelope",
  );
  const ready = s2ParseReady(readyResult);
  requireCondition(
    canonicalJson(ready.tokens) ===
      canonicalJson(readyRecord.facts.tokens),
    "host.s2_semantic",
    "semantic checker READY token projection differs",
  );
  S2_LEGS.forEach((leg, index) => {
    requireCondition(
      rubyRecordsByLeg.get(leg).facts.token ===
        ready.tokens[index],
      "host.s2_semantic",
      `semantic checker Ruby ${leg} token differs`,
    );
  });
  const readyEnvelopeValue = parseS2JsonFrame(
    readyEnvelope,
    "semantic READY envelope",
  );
  requireS2EnvelopeShape(
    readyEnvelopeValue,
    "semantic READY",
  );
  requireCondition(
    readyEnvelopeValue.sequence === 0 &&
      readyEnvelopeValue.kind === "READY" &&
      readyEnvelopeValue.workerIntentBase64 === null &&
      readyEnvelopeValue.supervisorCommandBase64 === null &&
      readyEnvelopeValue.supervisorCommandSha256 === null &&
      readyEnvelopeValue.supervisorResultBase64 ===
        readyResult.toString("base64") &&
      readyEnvelopeValue.supervisorResultSha256 ===
        sha256(readyResult),
    "host.s2_semantic",
    "semantic checker READY envelope relation differs",
  );
  const events = parsed.records.filter(
    (record) =>
      record.kind === "supervisor.transition" ||
      (record.kind === "supervisor.lifecycle" &&
        record.facts.lifecycle !== "READY"),
  );
  const expectedEvents = [];
  for (const [legIndex, leg] of S2_LEGS.entries()) {
    expectedEvents.push([
      "LEADER_OWNED",
      leg,
      ready.tokens[legIndex],
    ]);
    for (const transition of [
      ...S2_TRANSITION_PLAN[leg],
      "TERMINAL_KILL_PROBE_13",
      "TERMINAL_KILL_PROBE_14",
    ]) {
      expectedEvents.push([
        transition,
        leg,
        ready.tokens[legIndex],
      ]);
    }
    expectedEvents.push([
      "LEADER_REAPED",
      leg,
      ready.tokens[legIndex],
    ]);
  }
  expectedEvents.push(["CLOSEOUT", null, null]);
  requireCondition(
    events.length === 27 &&
      expectedEvents.length === 27,
    "host.s2_semantic",
    "semantic checker event cardinality differs",
  );
  let priorEventRecord = readyRecord;
  let priorEnvelopeSha256 = sha256(readyEnvelope);
  const generationStates = new Map(
    ready.tokens.map((token, index) => [
      token,
      {
        leg: S2_LEGS[index],
        started: false,
        sealed: false,
        transitions: 0,
      },
    ]),
  );
  events.forEach((record, index) => {
    const relay = record.facts.relay;
    const rawIntent = decodeS2RawFact(
      relay.workerIntent,
      `worker intent ${index}`,
      S2_PROTOCOL.frameBytes,
    );
    const rawCommand = decodeS2RawFact(
      relay.supervisorCommand,
      `supervisor command ${index}`,
      S2_PROTOCOL.frameBytes,
    );
    const rawResult = decodeS2RawFact(
      relay.supervisorResult,
      `supervisor result ${index}`,
      S2_PROTOCOL.frameBytes,
    );
    const rawEnvelope = decodeS2RawFact(
      relay.nodeEnvelope,
      `Node envelope ${index}`,
    );
    const intent = parseS2JsonFrame(
      rawIntent,
      `semantic worker intent ${index}`,
    );
    requireS2IntentShape(
      intent,
      `semantic worker ${index}`,
    );
    requireCondition(
      intent.sequence === index &&
        intent.ack.evidenceSequence ===
          priorEventRecord.sequence &&
        intent.ack.evidenceHash ===
          priorEventRecord.hash &&
        intent.ack.envelopeSha256 ===
          priorEnvelopeSha256 &&
        rawCommand.equals(s2SupervisorCommand(intent)),
      "host.s2_semantic",
      "semantic checker relay acknowledgement/command differs",
      { index },
    );
    const parsedResult = parseS2SupervisorFrame(rawResult);
    s2ValidateSupervisorResult(
      parsedResult,
      intent,
      index + 1,
      expectedEvents[index]?.[1] ?? undefined,
    );
    const envelope = parseS2JsonFrame(
      rawEnvelope,
      `semantic Node envelope ${index}`,
    );
    requireS2EnvelopeShape(
      envelope,
      `semantic Node ${index}`,
    );
    requireCondition(
      envelope.sequence === index + 1 &&
        envelope.kind === intent.kind &&
        envelope.workerIntentBase64 ===
          rawIntent.toString("base64") &&
        envelope.supervisorCommandBase64 ===
          rawCommand.toString("base64") &&
        envelope.supervisorCommandSha256 ===
          sha256(rawCommand) &&
        envelope.supervisorResultBase64 ===
          rawResult.toString("base64") &&
        envelope.supervisorResultSha256 ===
          sha256(rawResult),
      "host.s2_semantic",
      "semantic checker Node envelope relation differs",
      { index },
    );
    const [expectedKind, expectedLeg, expectedToken] =
      expectedEvents[index];
    const expectedIntentKind =
      expectedKind === "LEADER_OWNED"
        ? "START_LEG"
        : expectedKind === "LEADER_REAPED"
          ? "FINAL_REAP"
          : expectedKind === "CLOSEOUT"
            ? "CLOSE"
            : expectedKind;
    const generation =
      expectedToken === null
        ? undefined
        : generationStates.get(expectedToken);
    requireCondition(
      intent.kind === expectedIntentKind &&
        intent.token === expectedToken &&
        (expectedIntentKind === "START_LEG"
          ? intent.leg === expectedLeg
          : intent.leg === null),
      "host.s2_semantic",
      "semantic checker command schedule differs",
      { index, expectedIntentKind, intent },
    );
    if (expectedIntentKind === "START_LEG") {
      requireCondition(
        generation !== undefined &&
          generation.leg === expectedLeg &&
          !generation.started &&
          !generation.sealed,
        "host.s2_semantic",
        "generation token start is not affine",
        { index, expectedLeg, expectedToken },
      );
      generation.started = true;
    } else if (expectedIntentKind === "FINAL_REAP") {
      requireCondition(
        generation !== undefined &&
          generation.started &&
          !generation.sealed &&
          generation.transitions ===
            S2_TRANSITION_PLAN[expectedLeg].length + 2,
        "host.s2_semantic",
        "generation token seal precedes its full transition plan",
        { index, expectedLeg, generation },
      );
      generation.sealed = true;
    } else if (expectedIntentKind === "CLOSE") {
      requireCondition(
        [...generationStates.values()].every(
          (state) => state.started && state.sealed,
        ),
        "host.s2_semantic",
        "CLOSE preceded an unsealed generation",
      );
    } else {
      requireCondition(
        generation !== undefined &&
          generation.started &&
          !generation.sealed,
        "host.s2_semantic",
        "transition used an unavailable generation",
        { index, expectedLeg, generation },
      );
      generation.transitions += 1;
    }
    if (record.kind === "supervisor.transition") {
      requireCondition(
        record.facts.transition === expectedKind &&
          record.facts.leg === expectedLeg &&
          record.facts.token === expectedToken &&
          record.facts.outcome ===
            s2ExpectedTransitionOutcome(expectedKind) &&
          intent.kind === expectedKind &&
          intent.token === expectedToken &&
          record.facts.proofOrdinal ===
            s2ExpectedTransitionOrdinal(
              expectedLeg,
              expectedKind,
            ) &&
          parsedResult.fields[4] === record.facts.outcome,
        "host.s2_semantic",
        "semantic checker transition projection differs",
        { index, expectedKind },
      );
      const proofRecord = proofRecords.find(
        (candidate) =>
          candidate.facts.leg === expectedLeg &&
          candidate.facts.ordinal ===
            record.facts.proofOrdinal,
      );
      requireCondition(
        proofRecord !== undefined &&
          canonicalJson(intent.proof) ===
            canonicalJson(
              s2IntentProof({
                proof: proofRecord.facts,
              }),
            ),
        "host.s2_semantic",
        "semantic checker transition proof link differs",
        { index },
      );
    } else {
      requireCondition(
        record.facts.lifecycle === expectedKind &&
          (expectedLeg === null ||
            (record.facts.leg === expectedLeg &&
              record.facts.token === expectedToken)) &&
          (expectedKind !== "LEADER_REAPED" ||
            (record.facts.terminalKind ===
              parsedResult.fields[3] &&
              String(record.facts.terminalCode) ===
                parsedResult.fields[4])) &&
          (expectedKind !== "LEADER_OWNED" ||
            (decodeS2RawFact(
              record.facts.startupReport,
              `semantic startup report ${index}`,
              S2_PROTOCOL.startupReportBytes,
            ).toString("base64") === parsedResult.fields[3] &&
              decodeS2RawFact(
                record.facts.startupRelease,
                `semantic startup release ${index}`,
                S2_PROTOCOL.startupReleaseBytes,
              ).toString("base64") ===
                parsedResult.fields[4] &&
              record.facts.reportEof === true &&
              record.facts.releaseEof === true)),
        "host.s2_semantic",
        "semantic checker lifecycle projection differs",
        { index, expectedKind },
      );
    }
    priorEventRecord = record;
    priorEnvelopeSha256 = sha256(rawEnvelope);
  });
  for (const [legIndex, leg] of S2_LEGS.entries()) {
    const rubyRecord = rubyRecordsByLeg.get(leg);
    const transitionHashes = events
      .filter(
        (record) =>
          record.kind === "supervisor.transition" &&
          record.facts.leg === leg,
      )
      .map((record) => record.hash);
    const finalReapRecord = events.find(
      (record) =>
        record.kind === "supervisor.lifecycle" &&
        record.facts.lifecycle === "LEADER_REAPED" &&
        record.facts.leg === leg,
    );
    requireCondition(
      finalReapRecord !== undefined &&
        rubyRecord.facts.legIndex === legIndex &&
        rubyRecord.facts.token === ready.tokens[legIndex] &&
        canonicalJson(rubyRecord.facts.proofOrdinals) ===
          canonicalJson(S2_PROOF_ORDINALS[leg]) &&
        canonicalJson(rubyRecord.facts.transitions) ===
          canonicalJson(transitionHashes) &&
        rubyRecord.facts.finalReapHash ===
          finalReapRecord.hash,
      "host.s2_semantic",
      `semantic checker Ruby ${leg} event linkage differs`,
    );
  }
  const rawFinalAck = invocation.rawFinalAck;
  const finalAck = parseS2JsonFrame(
    rawFinalAck,
    "semantic final ACK",
  );
  requireS2FinalAckShape(finalAck, "semantic");
  requireCondition(
    canonicalJson(finalAck) ===
        canonicalJson(invocation.finalAck) &&
      finalAck.schema === 1 &&
      finalAck.sequence === 27 &&
      finalAck.kind === "COMMIT_ACK" &&
      finalAck.evidenceSequence ===
        priorEventRecord.sequence &&
      finalAck.evidenceHash === priorEventRecord.hash &&
      finalAck.envelopeSha256 === priorEnvelopeSha256 &&
      invocation.supervisorResultEof === true &&
      invocation.workerRelayEof === true &&
      invocation.supervisorTerminal.error === null &&
      invocation.supervisorTerminal.code === 0 &&
      invocation.supervisorTerminal.signal === null &&
      invocation.workerTerminal.error === null &&
      invocation.workerTerminal.code === 0 &&
      invocation.workerTerminal.signal === null,
    "host.s2_semantic",
    "semantic checker final ACK/terminal relation differs",
  );
  const workerFinal = findEvidenceRecord(
    parsed,
    "worker.final",
  );
  requireExactKeys(
    workerFinal.facts,
    [
      "capacity",
      "counters",
      "evidenceSequenceBeforeCloseout",
      "finalStaticDigest",
      "outcome",
      "packet",
      "releases",
      "retained",
      "sourceSha256",
      "tombstone",
    ],
    "host.s2_semantic",
    "worker final",
  );
  requireCondition(
    workerFinal.facts.outcome ===
        "PROVISIONAL_PHASE_A0_CLEAN" &&
      workerFinal.facts.releases ===
        "CORE_REPLAY_AMENDMENT_ONLY" &&
      canonicalJson(workerFinal.facts.packet) ===
        canonicalJson(S2_PACKET) &&
      workerFinal.facts.sourceSha256 ===
        initialStatic.owner.sha256 &&
      workerFinal.facts.evidenceSequenceBeforeCloseout ===
        workerFinal.sequence &&
      canonicalJson(workerFinal.facts.counters) ===
        canonicalJson(expectedActual) &&
      canonicalJson(workerFinal.facts.capacity) ===
        canonicalJson(workerModel.facts.capacity) &&
      workerFinal.facts.finalStaticDigest ===
        initialStatic.digest &&
      canonicalJson(workerFinal.facts.tombstone) ===
        canonicalJson(tombstoneRecord.facts) &&
      canonicalJson(workerFinal.facts.retained) ===
        canonicalJson(["invocation", "evidence", "a0.jsonl"]),
    "host.s2_semantic",
    "semantic checker final model/counters differ",
  );
  return Object.freeze({
    outcome: "EVIDENCE_ACCEPTING_PROVISIONAL_ONLY",
    records: parsed.records.length,
    proofs: proofRecords.length,
    events: events.length + 1,
    finalSequence: priorEventRecord.sequence,
    finalHash: priorEventRecord.hash,
    finalAckSha256: sha256(rawFinalAck),
  });
}

class WorkerDeadlineOwner {
  #confirmed = false;

  constructor(argv) {
    requireCondition(
      argv.workerDeadline <= argv.outerDeadline,
      "host.worker_deadline",
      "worker deadline exceeds its inherited outer deadline",
      {
        outerDeadlineNs: argv.outerDeadline.toString(),
        workerDeadlineNs: argv.workerDeadline.toString(),
      },
    );
    this.outerDeadlineNs = argv.outerDeadline;
    this.workerDeadlineNs = argv.workerDeadline;
    this.workerRemainingNs = argv.workerRemaining;
    this.basisToken = argv.basisToken;
    this.deadline = new AbsoluteDeadline(
      "worker",
      this.workerDeadlineNs,
    );
  }

  confirm(evidence) {
    requireCondition(
      !this.#confirmed &&
        evidence.basisToken === this.basisToken &&
        evidence.outerDeadlineNs === this.outerDeadlineNs.toString() &&
        evidence.workerDeadlineNs === this.workerDeadlineNs.toString() &&
        evidence.workerRemainingNs === this.workerRemainingNs.toString(),
      "host.worker_deadline",
      "worker clock-basis evidence differs",
      {
        evidence,
        basisToken: this.basisToken,
        outerDeadlineNs: this.outerDeadlineNs.toString(),
        workerDeadlineNs: this.workerDeadlineNs.toString(),
      },
    );
    this.#confirmed = true;
  }

  requireConfirmed() {
    requireCondition(
      this.#confirmed,
      "host.worker_deadline",
      "worker deadline owner was consumed before evidence confirmation",
    );
    return this.deadline;
  }
}

function verifyWorkerArguments(args) {
  requireCondition(
    args.length === 11,
    "host.worker_argv",
    "worker internal argv count differs",
    { count: args.length },
  );
  const [
    runToken,
    invocation,
    rootDev,
    rootIno,
    evidenceDev,
    evidenceIno,
    initialChainField,
    outerDeadline,
    workerDeadline,
    workerRemaining,
    basisTokenField,
  ] = args;
  const initialChainMatch =
    /^chain:([a-f0-9]{64})$/u.exec(initialChainField);
  const basisTokenMatch =
    /^basis:([a-f0-9]{64})$/u.exec(basisTokenField);
  requireCondition(
    /^[a-f0-9]{64}$/u.test(runToken) &&
      /^\/private\/tmp\/marrow-vsq-a-[a-f0-9]{8}\.[A-Za-z0-9]{6}$/u.test(
        invocation,
      ) &&
      Buffer.byteLength(invocation) === 41 &&
      invocation.includes(
        `/marrow-vsq-a-${runToken.slice(0, 8)}.`,
      ) &&
      initialChainMatch !== null &&
      basisTokenMatch !== null,
    "host.worker_argv",
    "worker token/path/chain spelling differs",
  );
  const initialChain = initialChainMatch[1];
  const basisToken = basisTokenMatch[1];
  const parsed = Object.freeze({
    runToken,
    invocation,
    rootDev: parseUnsignedBigInt(rootDev).toString(),
    rootIno: parseUnsignedBigInt(rootIno).toString(),
    evidenceDev: parseUnsignedBigInt(evidenceDev).toString(),
    evidenceIno: parseUnsignedBigInt(evidenceIno).toString(),
    initialChain,
    outerDeadline: parseUnsignedBigInt(
      outerDeadline,
      1n,
      MAX_U64,
    ),
    workerDeadline: parseUnsignedBigInt(
      workerDeadline,
      1n,
      MAX_U64,
    ),
    workerRemaining: parseUnsignedBigInt(
      workerRemaining,
      1n,
      MAX_U64,
    ),
    basisToken,
  });
  requireCondition(
    parsed.workerRemaining === 420_000_000_000n &&
      parsed.workerDeadline <= parsed.outerDeadline,
    "host.worker_deadline",
    "worker outer/remaining argv differs",
    {
      outerDeadline,
      workerDeadline,
      workerRemaining,
    },
  );
  return parsed;
}

function verifyWorkerRoots(argv, deadline) {
  const invocation = rootFact(argv.invocation, "invocation");
  requireCondition(
    invocation.dev === argv.rootDev &&
      invocation.ino === argv.rootIno,
    "host.worker_root",
    "worker invocation root identity differs",
    { invocation, argvRoot: [argv.rootDev, argv.rootIno] },
  );
  const evidenceRootPath = join(argv.invocation, "evidence");
  const evidenceRoot = rootFact(evidenceRootPath, "evidence-root");
  const evidencePath = join(evidenceRootPath, "a0.jsonl");
  const evidence = readBoundedRegular(
    evidencePath,
    EVIDENCE_MAX_BYTES,
    deadline,
  );
  requireCondition(
    evidence.identity.dev === argv.evidenceDev &&
      evidence.identity.ino === argv.evidenceIno,
    "host.worker_root",
    "worker evidence identity differs",
    {
      evidence: evidence.identity,
      argvEvidence: [argv.evidenceDev, argv.evidenceIno],
    },
  );
  const parsed = parseEvidence(evidence.body);
  requireCondition(
    parsed.previousHash === argv.initialChain,
    "host.worker_chain",
    "worker initial evidence chain differs",
    {
      parsed: parsed.previousHash,
      argv: argv.initialChain,
    },
  );
  return Object.freeze({
    invocation,
    evidenceRoot,
    evidencePath,
    evidenceIdentity: evidence.identity,
    parsed,
  });
}

function exactResidual(worker) {
  requireCondition(
    absentNoFollow(worker.preflight) &&
      absentNoFollow(worker.preflight) &&
      sameRoot(worker.evidenceRoot.path, worker.evidenceRoot) &&
      sameRoot(worker.invocation, worker.invocationReceipt),
    "host.closeout",
    "execution roots did not retire to the retained boundary",
  );
  const invocationNames = readdirSync(worker.invocation);
  const evidenceNames = readdirSync(worker.evidenceRoot.path);
  requireCondition(
    canonicalJson(invocationNames) === canonicalJson(["evidence"]) &&
      canonicalJson(evidenceNames) === canonicalJson(["a0.jsonl"]),
    "host.closeout",
    "retained invocation tree contains an unexpected entry",
    { invocationNames, evidenceNames },
  );
}

async function runWorkerBody(argv, deadlineOwner) {
  requireCondition(
    process.execPath === NODE &&
      process.argv0 === NODE &&
      process.version === "v24.16.0" &&
      process.platform === "darwin" &&
      process.arch === "arm64",
    "host.worker_runtime",
    "worker Node runtime identity differs",
  );
  const entryNowNs = process.hrtime.bigint();
  const entryRemainingNs = argv.workerDeadline - entryNowNs;
  requireCondition(
    entryRemainingNs >= 1n &&
      entryRemainingNs <= argv.workerRemaining,
    "host.worker_deadline",
    "worker entry remaining deadline differs",
    {
      entryNowNs: entryNowNs.toString(),
      deadlineNs: argv.workerDeadline.toString(),
      remainingNs: entryRemainingNs.toString(),
    },
  );
  const deadline = deadlineOwner.deadline;
  const roots = verifyWorkerRoots(
    argv,
    deadline.sub("entry-roots", DEADLINE_MS.workerEntry),
  );
  const initialRecord = findEvidenceRecord(
    roots.parsed,
    "supervisor.initial_static",
  );
  const preSpawnRecord = findEvidenceRecord(
    roots.parsed,
    "supervisor.pre_spawn",
  );
  deadlineOwner.confirm(preSpawnRecord.facts);
  deadlineOwner.requireConfirmed();
  const ownerEntry = streamOwner(
    deadline.sub("entry-owner", DEADLINE_MS.workerEntry),
    initialRecord.facts.static.owner,
  );
  const evidence = openEvidenceAppend(
    roots.evidencePath,
    roots.evidenceIdentity,
    roots.parsed,
    deadline,
  );
  evidence.add(
    "static",
    "worker.entry",
    {
      pid: process.pid,
      ppid: process.ppid,
      entryNowNs: entryNowNs.toString(),
      entryRemainingNs: entryRemainingNs.toString(),
      owner: ownerEntry,
      initialChain: argv.initialChain,
      basisToken: argv.basisToken,
    },
    deadline,
  );
  const capacity = new CapacityLedger({
    directories: 2,
    regularFiles: 1,
  });
  const worker = {
    deadline,
    invocation: argv.invocation,
    invocationReceipt: roots.invocation,
    evidenceRoot: roots.evidenceRoot,
    evidencePath: roots.evidencePath,
    capacity,
    counters: capacity.completed,
    fifoFacts: [],
  };
  const tombstones = new Tombstones();
  const preflight = join(argv.invocation, "preflight");
  const preflightReceipt = createDirectory(
    preflight,
    "preflight",
    worker.capacity,
  );
  worker.preflight = preflight;
  worker.preflightReceipt = preflightReceipt;
  const fifoRootPath = join(preflight, "fifo");
  const fifoRoot = createDirectory(
    fifoRootPath,
    "fifo-root",
    worker.capacity,
  );
  const canary = createCanary(
    worker,
    preflightReceipt,
    deadline,
  );
  evidence.add(
    "roots",
    "worker.preflight",
    {
      preflight: {
        pathHash: preflightReceipt.pathHash,
        dev: preflightReceipt.dev,
        ino: preflightReceipt.ino,
      },
      fifo: {
        pathHash: fifoRoot.pathHash,
        dev: fifoRoot.dev,
        ino: fifoRoot.ino,
      },
      canary: {
        pathHash: canary.pathHash,
        dev: canary.identity.dev,
        ino: canary.identity.ino,
        sha256: sha256(CANARY_BYTES),
      },
    },
    deadline,
  );
  const fifo = new FifoManager(
    worker,
    fifoRoot,
    tombstones,
    evidence,
  );
  let previousNode = await runNodeLeg(
    worker,
    fifo,
    evidence,
    tombstones,
    true,
  );
  previousNode = await runNodeLeg(
    worker,
    fifo,
    evidence,
    tombstones,
    false,
    previousNode,
  );
  const rubyContainerPath = join(preflight, "ruby");
  const rubyContainer = createDirectory(
    rubyContainerPath,
    "ruby-container",
    worker.capacity,
  );
  const proofOwner = new RubyProofOwner(
    worker,
    fifo,
    evidence,
    tombstones,
    canary,
  );
  let previousRuby;
  const legResults = [];
  for (const [legIndex, leg] of [
    "support",
    "denial",
    "one-receipt",
    "parent-loss",
  ].entries()) {
    const result = await runRubyLeg(
      worker,
      fifo,
      proofOwner,
      evidence,
      tombstones,
      legIndex,
      leg,
      rubyContainer,
      previousRuby,
    );
    previousRuby = result.rootReceipts;
    legResults.push(result);
  }
  removeDirectory(rubyContainerPath, rubyContainer);
  evidence.add(
    "fifo_facts",
    "fifo.lifecycle",
    {
      count: worker.fifoFacts.length,
      facts: worker.fifoFacts,
      digest: sha256(Buffer.from(canonicalJson(worker.fifoFacts))),
    },
    deadline,
  );
  const capacitySnapshot = worker.capacity.snapshot();
  requireCondition(
    worker.counters.fifoBatches ===
      worker.counters.psCaptures + 6 &&
      worker.counters.fifoInodes ===
        worker.counters.psCaptures * 2 + 16 &&
      worker.counters.proofs === worker.counters.psCaptures &&
      worker.counters.nodeLegs === 2 &&
      worker.counters.rubyLegs === 4 &&
      worker.counters.directories === 69 &&
      worker.counters.regularFiles === 2 &&
      worker.counters.fifoPathBytes === 6_428 &&
      worker.counters.sockets === 7 &&
      canonicalJson(capacitySnapshot.reserved) ===
        canonicalJson(capacitySnapshot.completed) &&
      worker.counters.fifoBatches <= LIMITS.fifoBatches &&
      worker.counters.fifoInodes <= LIMITS.fifoInodes &&
      worker.fifoFacts.length === worker.counters.fifoInodes &&
      proofOwner.nextBatch === worker.counters.psCaptures &&
      proofOwner.absenceOraclePromoted,
    "host.counter_model",
    "Phase A0 actual/capacity counters differ",
    worker.counters,
  );
  removeDirectory(fifoRootPath, fifoRoot);
  retireCanary(canary, deadline);
  removeDirectory(preflight, preflightReceipt);
  exactResidual(worker);
  const finalStatic = staticPass(
    deadline.sub("final-static", DEADLINE_MS.finalStatic),
    initialRecord.facts.static,
  );
  const tombstone = tombstones.digest();
  evidence.add(
    "tombstone",
    "worker.tombstone",
    tombstone,
    deadline,
  );
  evidence.add(
    "envelope",
    "worker.model",
    {
      version: PACKET.version,
      actual: worker.counters,
      capacity: capacitySnapshot,
      descriptorPeak: LIMITS.protocolDescriptors,
      taskFifoPeak: LIMITS.taskFifoDescriptors,
      legResults,
      tombstone,
      finalStaticDigest: finalStatic.digest,
    },
    deadline,
  );
  const finalRecord = evidence.add(
    "closeout",
    "worker.final",
    {
      outcome: "PROVISIONAL_PHASE_A0_CLEAN",
      releases: "A1_PACKET_ONLY",
      sourceSha256: finalStatic.owner.sha256,
      packet: PACKET,
      evidenceSequence: evidence.sequence + 1,
      counters: worker.counters,
      capacity: capacitySnapshot,
      finalStaticDigest: finalStatic.digest,
      tombstone,
      retained: ["invocation", "evidence", "a0.jsonl"],
    },
    deadline.sub("artifact-closeout", DEADLINE_MS.artifactCloseout),
  );
  exactResidual(worker);
  const snapshot = evidence.finish(
    deadline.sub("artifact-finish", DEADLINE_MS.artifactCloseout),
  );
  requireCondition(
    snapshot.previousHash === finalRecord.hash,
    "host.evidence",
    "final evidence chain differs at worker close",
  );
}

async function runS2WorkerBody(argv, deadlineOwner) {
  requireCondition(
    process.execPath === NODE &&
      process.argv0 === NODE &&
      process.version === "v24.16.0" &&
      process.platform === "darwin" &&
      process.arch === "arm64",
    "host.worker_runtime",
    "S2 worker Node runtime identity differs",
  );
  const entryNowNs = process.hrtime.bigint();
  const entryRemainingNs = argv.workerDeadline - entryNowNs;
  requireCondition(
    entryRemainingNs >= 1n &&
      entryRemainingNs <= argv.workerRemaining,
    "host.worker_deadline",
    "S2 worker entry remaining deadline differs",
    {
      entryNowNs: entryNowNs.toString(),
      deadlineNs: argv.workerDeadline.toString(),
      remainingNs: entryRemainingNs.toString(),
    },
  );
  const deadline = deadlineOwner.deadline;
  const roots = verifyWorkerRoots(
    argv,
    deadline.sub("entry-roots", DEADLINE_MS.workerEntry),
  );
  const initialRecord = findEvidenceRecord(
    roots.parsed,
    "supervisor.initial_static",
  );
  const preSpawnRecord = findEvidenceRecord(
    roots.parsed,
    "supervisor.pre_spawn",
  );
  deadlineOwner.confirm(preSpawnRecord.facts);
  deadlineOwner.requireConfirmed();
  const ownerEntry = streamOwner(
    deadline.sub("entry-owner", DEADLINE_MS.workerEntry),
    initialRecord.facts.static.owner,
  );
  const evidence = openEvidenceAppend(
    roots.evidencePath,
    roots.evidenceIdentity,
    roots.parsed,
    deadline,
  );
  evidence.add(
    "static",
    "worker.entry",
    {
      pid: process.pid,
      ppid: process.ppid,
      entryNowNs: entryNowNs.toString(),
      entryRemainingNs: entryRemainingNs.toString(),
      owner: ownerEntry,
      initialChain: argv.initialChain,
      basisToken: argv.basisToken,
    },
    deadline,
  );
  const relayStream = new Socket({
    fd: 3,
    readable: true,
    writable: true,
  });
  activeS2WorkerRelayStream = relayStream;
  const relay = new S2RelayClient(relayStream, deadline);
  const ready = await relay.receiveReady();
  const readyRecord = evidence.add(
    "supervisor",
    "supervisor.lifecycle",
    {
      lifecycle: "READY",
      packet: S2_PACKET,
      tokens: ready.ready.tokens,
      supervisorResult: s2RawFact(
        ready.supervisorResult,
      ),
      nodeEnvelope: s2RawFact(ready.rawEnvelope),
    },
    deadline,
  );
  relay.acknowledge(readyRecord, ready.rawEnvelope);
  const capacity = new CapacityLedger({
    directories: 2,
    regularFiles: 1,
    rubyCustodySupervisors: 1,
    protocolSocketpairs: 3,
    protocolEndpoints: 6,
    startupPipes: 8,
    startupPipeEndpoints: 4,
    descriptorSlots: 59,
  });
  beginS2RootInventory(capacity, [
    roots.invocation,
    roots.evidenceRoot,
  ]);
  const worker = {
    deadline,
    invocation: argv.invocation,
    invocationReceipt: roots.invocation,
    evidenceRoot: roots.evidenceRoot,
    evidencePath: roots.evidencePath,
    capacity,
    counters: capacity.completed,
    fifoFacts: [],
  };
  const tombstones = new Tombstones();
  const preflight = join(argv.invocation, "preflight");
  const preflightReceipt = createDirectory(
    preflight,
    "preflight",
    worker.capacity,
  );
  worker.preflight = preflight;
  worker.preflightReceipt = preflightReceipt;
  const fifoRootPath = join(preflight, "fifo");
  const fifoRoot = createDirectory(
    fifoRootPath,
    "fifo-root",
    worker.capacity,
  );
  const canary = createCanary(
    worker,
    preflightReceipt,
    deadline,
  );
  evidence.add(
    "roots",
    "worker.preflight",
    {
      preflight: {
        pathHash: preflightReceipt.pathHash,
        dev: preflightReceipt.dev,
        ino: preflightReceipt.ino,
      },
      fifo: {
        pathHash: fifoRoot.pathHash,
        dev: fifoRoot.dev,
        ino: fifoRoot.ino,
      },
      canary: {
        pathHash: canary.pathHash,
        dev: canary.identity.dev,
        ino: canary.identity.ino,
        sha256: sha256(CANARY_BYTES),
      },
    },
    deadline,
  );
  const fifo = new FifoManager(
    worker,
    fifoRoot,
    tombstones,
    evidence,
  );
  let previousNode = await runNodeLeg(
    worker,
    fifo,
    evidence,
    tombstones,
    true,
  );
  previousNode = await runNodeLeg(
    worker,
    fifo,
    evidence,
    tombstones,
    false,
    previousNode,
  );
  const rubyContainerPath = join(preflight, "ruby");
  const rubyContainer = createDirectory(
    rubyContainerPath,
    "ruby-container",
    worker.capacity,
  );
  const proofOwner = new S2CaptureAttemptOwner(
    worker,
    fifo,
    evidence,
    tombstones,
    canary,
  );
  let previousRuby;
  const legResults = [];
  for (const [legIndex, leg] of S2_LEGS.entries()) {
    const result = await runS2RubyLeg({
      worker,
      fifo,
      proofOwner,
      evidence,
      tombstones,
      relay,
      token: ready.ready.tokens[legIndex],
      legIndex,
      leg,
      containerReceipt: rubyContainer,
      previousRoots: previousRuby,
    });
    previousRuby = result.rootReceipts;
    legResults.push(result);
  }
  removeDirectory(rubyContainerPath, rubyContainer);
  evidence.add(
    "fifo_facts",
    "fifo.lifecycle",
    {
      count: worker.fifoFacts.length,
      facts: worker.fifoFacts,
      digest: sha256(
        Buffer.from(canonicalJson(worker.fifoFacts)),
      ),
    },
    deadline,
  );
  const capacitySnapshot = worker.capacity.snapshot();
  const rootInventory = snapshotS2RootInventory(
    worker.capacity,
  );
  requireCondition(
    worker.counters.captureAttempts === 36 &&
      worker.counters.proofs === 36 &&
      worker.counters.psCaptures === 36 &&
      worker.counters.fifoBatches === 42 &&
      worker.counters.fifoInodes === 88 &&
      worker.counters.fifoPathBytes === 6_428 &&
      worker.counters.directories === 69 &&
      worker.counters.regularFiles === 2 &&
      worker.counters.nodeLegs === 2 &&
      worker.counters.rubyLegs === 4 &&
      worker.counters.sockets === 7 &&
      worker.counters.rubyCustodySupervisors === 1 &&
      worker.counters.protocolSocketpairs === 3 &&
      worker.counters.protocolEndpoints === 6 &&
      worker.counters.startupPipes === 8 &&
      worker.counters.startupPipeEndpoints === 4 &&
      worker.counters.descriptorSlots === 59 &&
      worker.fifoFacts.length === 88 &&
      proofOwner.reservationOwner.nextBatch === 36 &&
      proofOwner.reservationOwner.attemptCount === 36 &&
      proofOwner.reservationOwner.replacementUsed === false &&
      canonicalJson(capacitySnapshot.reserved) ===
        canonicalJson(capacitySnapshot.completed),
    "host.counter_model",
    "S2 Phase A0 exact clean counters differ",
    {
      counters: worker.counters,
      fifoFacts: worker.fifoFacts.length,
      attempts: proofOwner.reservationOwner.attemptCount,
    },
  );
  removeDirectory(fifoRootPath, fifoRoot);
  retireCanary(canary, deadline);
  removeDirectory(preflight, preflightReceipt);
  exactResidual(worker);
  const finalStatic = staticPass(
    deadline.sub("final-static", DEADLINE_MS.finalStatic),
    initialRecord.facts.static,
  );
  const tombstone = tombstones.digest();
  evidence.add(
    "tombstone",
    "worker.tombstone",
    tombstone,
    deadline,
  );
  evidence.add(
    "envelope",
    "worker.model",
    {
      version: S2_PACKET.version,
      actual: worker.counters,
      capacity: capacitySnapshot,
      descriptorPeak: LIMITS.protocolDescriptors,
      taskFifoPeak: LIMITS.taskFifoDescriptors,
      protocol: S2_PROTOCOL,
      legResults,
      rootInventory,
      tombstone,
      finalStaticDigest: finalStatic.digest,
    },
    deadline,
  );
  evidence.add(
    "closeout",
    "worker.final",
    {
      outcome: "PROVISIONAL_PHASE_A0_CLEAN",
      releases: "CORE_REPLAY_AMENDMENT_ONLY",
      sourceSha256: finalStatic.owner.sha256,
      packet: S2_PACKET,
      evidenceSequenceBeforeCloseout: evidence.sequence,
      counters: worker.counters,
      capacity: capacitySnapshot,
      finalStaticDigest: finalStatic.digest,
      tombstone,
      retained: ["invocation", "evidence", "a0.jsonl"],
    },
    deadline.sub(
      "artifact-closeout",
      DEADLINE_MS.artifactCloseout,
    ),
  );
  exactResidual(worker);
  const close = await relay.command("CLOSE", null, null);
  const closeoutRecord = evidence.add(
    "supervisor",
    "supervisor.lifecycle",
    {
      lifecycle: "CLOSEOUT",
      packet: S2_PACKET,
      relay: s2RelayEvidenceFacts(close),
      expectedEvidenceRecords:
        S2_PROTOCOL.cleanEvidenceRecords,
    },
    deadline,
  );
  requireCondition(
    evidence.sequence ===
        S2_PROTOCOL.cleanEvidenceRecords &&
      closeoutRecord.sequence ===
        S2_PROTOCOL.cleanEvidenceRecords - 1,
    "host.evidence",
    "S2 clean evidence cardinality differs",
    {
      sequence: evidence.sequence,
      closeoutSequence: closeoutRecord.sequence,
    },
  );
  const snapshot = evidence.finish(
    deadline.sub(
      "artifact-finish",
      DEADLINE_MS.artifactCloseout,
    ),
  );
  requireCondition(
    snapshot.previousHash === closeoutRecord.hash,
    "host.evidence",
    "S2 final evidence chain differs at worker close",
  );
  await relay.finalAck(closeoutRecord, close.rawEnvelope);
  activeS2WorkerRelayStream = undefined;
}

async function runS2Worker(args) {
  const argv = verifyWorkerArguments(args);
  const deadlineOwner = new WorkerDeadlineOwner(argv);
  try {
    await runS2WorkerBody(argv, deadlineOwner);
  } catch (error) {
    const relayToClose = activeS2WorkerRelayStream;
    activeS2WorkerRelayStream = undefined;
    let reportingError;
    try {
      const path = join(argv.invocation, "evidence", "a0.jsonl");
      const reportDeadline = deadlineOwner.deadline;
      const retained = readBoundedRegular(
        path,
        EVIDENCE_MAX_BYTES,
        reportDeadline,
      );
      requireCondition(
        retained.identity.dev === argv.evidenceDev &&
          retained.identity.ino === argv.evidenceIno,
        "host.worker_failure",
        "worker failure evidence identity differs",
      );
      const parsed = parseEvidence(retained.body);
      const existingStops = parsed.records.filter(
        (record) => record.kind === "terminal.stop",
      );
      requireCondition(
        existingStops.length === 0 &&
          parsed.records.length <=
            S2_PROTOCOL.retainedFaultEvidenceRecords,
        "host.worker_failure",
        "worker failure chain already contains a terminal STOP",
        {
          records: parsed.records.length,
          terminalStops: existingStops.length,
        },
      );
      const cleanPostChain =
        parsed.records.length ===
          S2_PROTOCOL.cleanEvidenceRecords &&
        parsed.records.at(-1)?.kind ===
          "supervisor.lifecycle" &&
        parsed.records.at(-1)?.facts.lifecycle === "CLOSEOUT";
      if (!cleanPostChain) {
        requireCondition(
          parsed.records.length <
            S2_PROTOCOL.retainedFaultEvidenceRecords,
          "host.worker_failure",
          "worker fault chain lacks terminal STOP capacity",
          { records: parsed.records.length },
        );
        const writer = openEvidenceAppend(
          path,
          retained.identity,
          parsed,
          reportDeadline,
        );
        const owner = streamOwner(reportDeadline);
        writer.add(
          "closeout",
          "terminal.stop",
          {
            outcome: "STOP",
            pid: process.pid,
            sourceSha256: owner.sha256,
            fault: safeError(error),
            relay:
              error instanceof HostAuthorityError &&
              error.code === "host.s2_node_stop"
                ? error.data
                : null,
            stateRetained: true,
          },
          reportDeadline,
        );
        writer.finish(reportDeadline);
      } else {
        requireCondition(
          cleanPostChain,
          "host.worker_failure",
          "post-chain failure lacks the exact clean-chain prefix",
        );
      }
    } catch (secondary) {
      reportingError = secondary;
    }
    if (reportingError !== undefined) {
      relayToClose?.destroy();
      throw new AggregateError(
        [error, reportingError],
        "worker operation and typed failure reporting both failed",
      );
    }
    relayToClose?.destroy();
    throw error;
  }
}

async function runWorker(args) {
  return runS2Worker(args);
}

async function waitForWorker(
  launched,
  workerDeadlineNs,
  outerDeadline,
) {
  try {
    const terminal = await waitFor(
      outerDeadline.atMost("worker-normal", workerDeadlineNs),
      launched.terminal.promise,
      "host.worker_timeout",
      "worker did not finish by its inherited deadline",
    );
    return Object.freeze({
      terminal,
      forced: false,
      actions: Object.freeze([]),
    });
  } catch (error) {
    if (!(error instanceof HostAuthorityError) ||
        error.code !== "host.worker_timeout") {
      throw error;
    }
  }
  outerDeadline.requireReserve(15_000, "host.worker_timeout");
  requireCondition(
    launched.terminal.current() === null,
    "host.worker_timeout",
    "worker terminal raced its forced-reap witness",
  );
  const actions = [];
  const term = Object.freeze({
    pid: launched.pid,
    nonce: launched.nonce,
    signal: "SIGTERM",
    monotonicNs: process.hrtime.bigint().toString(),
  });
  requireCondition(
    launched.child.kill("SIGTERM"),
    "host.worker_timeout",
    "worker TERM was refused",
    term,
  );
  actions.push(term);
  try {
    const terminal = await waitFor(
      outerDeadline.sub("worker-term", 5_000),
      launched.terminal.promise,
      "host.worker_timeout",
      "worker did not reap after TERM",
    );
    return Object.freeze({
      terminal,
      forced: true,
      actions: Object.freeze(actions),
    });
  } catch (error) {
    if (!(error instanceof HostAuthorityError) ||
        error.code !== "host.worker_timeout") {
      throw error;
    }
  }
  requireCondition(
    launched.terminal.current() === null,
    "host.worker_timeout",
    "worker terminal raced its KILL witness",
  );
  const kill = Object.freeze({
    pid: launched.pid,
    nonce: launched.nonce,
    signal: "SIGKILL",
    monotonicNs: process.hrtime.bigint().toString(),
  });
  requireCondition(
    launched.child.kill("SIGKILL"),
    "host.worker_timeout",
    "worker KILL was refused",
    kill,
  );
  actions.push(kill);
  const terminal = await waitFor(
    outerDeadline.sub("worker-kill-reap", 5_000),
    launched.terminal.promise,
    "host.worker_timeout",
    "worker did not reap after KILL",
  );
  return Object.freeze({
    terminal,
    forced: true,
    actions: Object.freeze(actions),
  });
}

function emitResult(value, deadline) {
  requireCondition(
    deadline instanceof AbsoluteDeadline,
    "host.result_deadline",
    "result emission lacks its inherited outer deadline",
  );
  deadline.check(
    "host.result_deadline",
    "result emission began after its inherited outer deadline",
  );
  const encoded = Buffer.from(`${canonicalJson(value)}\n`);
  requireCondition(
    encoded.length <= RESULT_MAX_BYTES,
    "host.result_capacity",
    "supervisor result exceeded its byte ceiling",
    { bytes: encoded.length },
  );
  writeAll(1, encoded, deadline);
  deadline.check(
    "host.result_deadline",
    "result emission returned after its inherited outer deadline",
  );
}

async function runSupervisor() {
  requireCondition(
    process.execPath === NODE &&
      process.argv0 === NODE &&
      process.argv.length === 2,
    "host.supervisor_runtime",
    "supervisor invocation/runtime differs",
    {
      execPath: process.execPath,
      argv0: process.argv0,
      argvCount: process.argv.length,
    },
  );
  const startNs = process.hrtime.bigint();
  const outerDeadlineNs =
    startNs + 660_000_000_000n;
  const outerDeadline = new AbsoluteDeadline(
    "supervisor",
    outerDeadlineNs,
  );
  const initialStatic = staticPass(
    outerDeadline.sub("initial-static", DEADLINE_MS.initialStatic),
  );
  const setupDeadline = outerDeadline.sub(
    "setup",
    DEADLINE_MS.supervisorSetup,
  );
  const runToken = randomToken();
  const state = createInvocation(
    runToken,
    setupDeadline,
    new CapacityLedger(),
  );
  const evidenceFile = createEvidenceFile(state, setupDeadline);
  evidenceFile.writer.add(
    "static",
    "supervisor.initial_static",
    {
      static: initialStatic,
      startNs: startNs.toString(),
      outerDeadlineNs: outerDeadlineNs.toString(),
    },
    setupDeadline,
  );
  evidenceFile.writer.add(
    "envelope",
    "supervisor.setup",
    {
      version: PACKET.version,
      packet: PACKET,
      runToken,
      invocation: {
        pathHash: state.invocationReceipt.pathHash,
        dev: state.invocationReceipt.dev,
        ino: state.invocationReceipt.ino,
      },
      evidence: {
        dev: evidenceFile.identity.dev,
        ino: evidenceFile.identity.ino,
        nlink: evidenceFile.identity.nlink,
        pathHash: sha256(
          Buffer.from(evidenceFile.path),
        ),
      },
    },
    setupDeadline,
  );
  evidenceFile.writer.finish(setupDeadline);
  const preSpawnDeadline = outerDeadline.sub(
    "pre-spawn",
    DEADLINE_MS.preSpawnRecheck,
  );
  const nodeRecheck = streamRegular(
    NODE,
    FIXED_PINS[0],
    preSpawnDeadline,
    initialStatic.files[0],
  );
  const ownerRecheck = streamOwner(
    preSpawnDeadline,
    initialStatic.owner,
  );
  const preSpawnNowNs = process.hrtime.bigint();
  requireCondition(
    outerDeadlineNs - preSpawnNowNs >= 445_000_000_000n,
    "host.worker_deadline",
    "supervisor lacks its pre-spawn outer reserve",
    {
      remainingNs: (outerDeadlineNs - preSpawnNowNs).toString(),
      requiredNs: "445000000000",
    },
  );
  const workerDeadlineNs =
    preSpawnNowNs + 420_000_000_000n;
  const workerRemainingNs = workerDeadlineNs - preSpawnNowNs;
  const basisToken = randomToken();
  const initialBody = readBoundedRegular(
    evidenceFile.path,
    EVIDENCE_MAX_BYTES,
    preSpawnDeadline,
    evidenceFile.identity,
  );
  const initialParsed = parseEvidence(initialBody.body);
  const initialAppend = openEvidenceAppend(
    evidenceFile.path,
    evidenceFile.identity,
    initialParsed,
    preSpawnDeadline,
  );
  initialAppend.add(
    "static",
    "supervisor.pre_spawn",
    {
      node: nodeRecheck,
      owner: ownerRecheck,
      basisToken,
      preSpawnNowNs: preSpawnNowNs.toString(),
      outerDeadlineNs: outerDeadlineNs.toString(),
      workerDeadlineNs: workerDeadlineNs.toString(),
      workerRemainingNs: workerRemainingNs.toString(),
    },
    preSpawnDeadline,
  );
  const initialSnapshot = initialAppend.finish(preSpawnDeadline);
  const nulls = [
    openDevNull(fsConstants.O_RDONLY, preSpawnDeadline),
    openDevNull(fsConstants.O_WRONLY, preSpawnDeadline),
    openDevNull(fsConstants.O_WRONLY, preSpawnDeadline),
  ];
  const supervisorTombstones = new Tombstones();
  let launched;
  try {
    launched = spawnExact({
      executable: NODE,
      args: [
        OWNER_PATH,
        "--vsq-a0-worker",
        runToken,
        state.invocation,
        state.invocationReceipt.dev,
        state.invocationReceipt.ino,
        evidenceFile.identity.dev,
        evidenceFile.identity.ino,
        `chain:${initialSnapshot.previousHash}`,
        outerDeadlineNs.toString(),
        workerDeadlineNs.toString(),
        workerRemainingNs.toString(),
        `basis:${basisToken}`,
      ],
      cwd: state.invocation,
      env: closedEnvironment(state.invocation, state.invocation),
      stdio: nulls.map((entry) => entry.fd),
      label: "phase-a0-worker",
      tombstones: supervisorTombstones,
    });
  } finally {
    closeParentDescriptors(nulls.map((entry) => entry.fd));
  }
  const settled = await waitForWorker(
    launched,
    workerDeadlineNs,
    outerDeadline,
  );
  if (settled.forced) {
    emitResult({
      code: "WORKER_FORCED_REAP_STOP",
      outcome: "STOP",
      workerPid: launched.pid,
      terminal: settled.terminal,
      actions: settled.actions,
      stateRetained: true,
    }, outerDeadline);
    return 1;
  }
  requireExactTerminal(
    settled.terminal,
    "host.worker_terminal",
    "phase-a0-worker",
  );
  const postDeadline = outerDeadline.sub(
    "post-worker",
    DEADLINE_MS.supervisorPostWorker,
  );
  const finalEvidence = readBoundedRegular(
    evidenceFile.path,
    EVIDENCE_MAX_BYTES,
    postDeadline,
    evidenceFile.identity,
  );
  const parsed = parseEvidence(finalEvidence.body);
  const finalRecord = findEvidenceRecord(parsed, "worker.final");
  requireCondition(
    finalRecord.facts.outcome === "PROVISIONAL_PHASE_A0_CLEAN" &&
      finalRecord.facts.sourceSha256 === initialStatic.owner.sha256 &&
      sameRoot(state.invocation, state.invocationReceipt) &&
      sameRoot(state.evidenceRoot, state.evidenceRootReceipt) &&
      canonicalJson(readdirSync(state.invocation)) ===
        canonicalJson(["evidence"]) &&
      canonicalJson(readdirSync(state.evidenceRoot)) ===
        canonicalJson(["a0.jsonl"]),
    "host.closeout",
    "supervisor final evidence/root verification differs",
    { finalRecord: finalRecord.facts },
  );
  emitResult({
    code: "host.phase_a0_provisional_clean",
    outcome: "PROVISIONAL_PHASE_A0_CLEAN",
    releases: "A1_PACKET_ONLY",
    sourceSha256: initialStatic.owner.sha256,
    packet: {
      designSha256: PACKET.design.sha256,
      checkerSha256: PACKET.checker.sha256,
      manifestSha256: PACKET.manifestSha256,
      pathNulSha256: PACKET.pathNul.sha256,
    },
    evidence: {
      pathHash: sha256(Buffer.from(evidenceFile.path)),
      dev: finalEvidence.identity.dev,
      ino: finalEvidence.identity.ino,
      nlink: finalEvidence.identity.nlink,
      bytes: finalEvidence.bytes,
      sha256: finalEvidence.sha256,
      chain: parsed.previousHash,
    },
    counters: finalRecord.facts.counters,
    finalStaticDigest: finalRecord.facts.finalStaticDigest,
    tombstone: finalRecord.facts.tombstone,
    retainedInvocation: state.invocation,
  }, outerDeadline);
  return 0;
}

function completeS2ProtocolReservations(capacity, reservations) {
  for (const receipt of [
    reservations.supervisor,
    reservations.socketpairs,
    reservations.endpoints,
    reservations.startupPipes,
    reservations.startupPipeEndpoints,
    reservations.descriptorSlots,
  ]) {
    capacity.complete(receipt);
  }
}

function openS2NullSet(custody, flags, deadline) {
  const entries = [];
  for (const flag of flags) {
    const entry = openDevNull(flag, deadline);
    custody.trackDescriptor(entry);
    entries.push(entry);
  }
  return Object.freeze(entries);
}

function openS2ProtocolBootstrap(
  state,
  custody,
  deadline,
) {
  requireProtocolPeak(S2_DESCRIPTOR_CAPACITY);
  const reservations = reserveS2Protocol(state.capacity);
  const [supervisorNull] = openS2NullSet(
    custody,
    [fsConstants.O_WRONLY],
    deadline,
  );
  return Object.freeze({ reservations, supervisorNull });
}

class S2OuterCustody {
  constructor(
    outerDeadline,
    closeChildren = closeS2ChildrenAfterFault,
  ) {
    requireCondition(
      outerDeadline instanceof AbsoluteDeadline,
      "host.s2_outer_custody",
      "outer custody lacks its inherited deadline",
    );
    requireCondition(
      typeof closeChildren === "function",
      "host.s2_outer_custody",
      "outer custody lacks its typed closeout owner",
    );
    this.outerDeadline = outerDeadline;
    this.closeChildren = closeChildren;
    this.descriptors = new Map();
    this.supervisor = undefined;
    this.supervisorReader = undefined;
    this.worker = undefined;
    this.entered = false;
    this.closed = false;
  }

  trackDescriptor(entry) {
    requireCondition(
      entry !== null &&
        typeof entry === "object" &&
        Number.isInteger(entry.fd) &&
        !this.descriptors.has(entry.fd),
      "host.s2_outer_custody",
      "outer custody descriptor registration differs",
    );
    this.descriptors.set(entry.fd, entry);
  }

  closeDescriptors(entries, deadline) {
    const errors = [];
    for (const entry of entries) {
      requireCondition(
        this.descriptors.get(entry.fd) === entry,
        "host.s2_outer_custody",
        "outer custody descriptor owner differs",
        { fd: entry.fd },
      );
      try {
        checkedClose(entry.fd, deadline);
        this.descriptors.delete(entry.fd);
      } catch (error) {
        errors.push(error);
      }
    }
    if (errors.length > 0) {
      throw new AggregateError(
        errors,
        "outer custody descriptor closeout failed",
      );
    }
  }

  bindSupervisor(supervisor, reader = undefined) {
    requireCondition(
      this.supervisor === undefined ||
        this.supervisor === supervisor,
      "host.s2_outer_custody",
      "outer custody supervisor was rebound",
    );
    this.supervisor = supervisor;
    if (reader !== undefined) {
      this.supervisorReader = reader;
    }
  }

  bindWorker(worker) {
    requireCondition(
      this.worker === undefined || this.worker === worker,
      "host.s2_outer_custody",
      "outer custody worker was rebound",
    );
    this.worker = worker;
  }

  async run(operation) {
    requireCondition(
      !this.entered && !this.closed,
      "host.s2_outer_custody",
      "outer custody was entered more than once",
    );
    this.entered = true;
    try {
      const value = await operation(this);
      requireCondition(
        this.descriptors.size === 0 &&
          (this.worker === undefined ||
            this.worker.terminal.current() !== null) &&
          (this.supervisor === undefined ||
            this.supervisor.terminal.current() !== null),
        "host.s2_outer_custody",
        "outer custody clean return retained live owners",
        {
          descriptors: [...this.descriptors.keys()],
          workerTerminal:
            this.worker?.terminal.current() ?? null,
          supervisorTerminal:
            this.supervisor?.terminal.current() ?? null,
        },
      );
      this.closed = true;
      return value;
    } catch (firstFault) {
      const cleanupStartNs = process.hrtime.bigint();
      const secondaryFaults = [];
      for (const entry of [...this.descriptors.values()]) {
        try {
          checkedClose(entry.fd, this.outerDeadline);
          this.descriptors.delete(entry.fd);
        } catch (error) {
          secondaryFaults.push(error);
        }
      }
      let closeout;
      try {
        closeout = await this.closeChildren(
          this.supervisor,
          this.supervisorReader,
          this.worker,
          this.outerDeadline,
          cleanupStartNs,
        );
        secondaryFaults.push(...closeout.secondary);
      } catch (error) {
        secondaryFaults.push(error);
      }
      this.closed = true;
      throw new HostAuthorityError(
        typeof firstFault?.code === "string"
          ? firstFault.code
          : "host.s2_outer_fault",
        String(
          firstFault?.message ??
            "S2 outer operation selected retained STOP",
        ),
        {
          firstFault: safeError(firstFault),
          secondaryFaults: secondaryFaults.map(safeError),
          cleanupStartNs: cleanupStartNs.toString(),
          closeout:
            closeout === undefined
              ? null
              : {
                  workerTerminal:
                    closeout.workerTerminal ?? null,
                  supervisorTerminal:
                    closeout.supervisorTerminal ?? null,
                  retainedSupervisorFrames:
                    closeout.retainedSupervisorFrames,
                },
          retained: true,
        },
      );
    }
  }
}

async function closeS2ChildrenAfterFault(
  supervisor,
  supervisorReader,
  worker,
  outerDeadline,
  cleanupStartNs,
) {
  requireCondition(
    typeof cleanupStartNs === "bigint" &&
      cleanupStartNs <= outerDeadline.endsNs,
    "host.s2_fault_closeout",
    "fault closeout lacks its first-fault monotonic witness",
  );
  const secondary = [];
  const retainedSupervisorFrames = [];
  try {
    worker?.child.stdio[3]?.destroy();
  } catch (error) {
    secondary.push(error);
  }
  try {
    supervisor?.child.stdin?.end();
  } catch (error) {
    secondary.push(error);
  }
  let workerTerminal;
  if (worker !== undefined) {
    try {
      workerTerminal = (
        await settleDirectChild(
        worker,
        outerDeadline,
          {
            normalMs: S2_FAULT_CLOSEOUT_MS.workerNormal,
            termMs: S2_FAULT_CLOSEOUT_MS.workerTerm,
            killMs: S2_FAULT_CLOSEOUT_MS.workerKill,
            label: "s2-worker-fault-closeout",
            allowSignal: true,
          },
        )
      ).terminal;
    } catch (error) {
      secondary.push(error);
    }
  }
  let supervisorTerminal;
  if (supervisor !== undefined) {
    if (
      supervisorReader !== undefined &&
      !supervisorReader.ended &&
      !supervisorReader.failed
    ) {
      try {
        while (true) {
          const frame = await supervisorReader.read();
          if (frame === null) break;
          retainedSupervisorFrames.push(s2RawFact(frame));
        }
      } catch (error) {
        secondary.push(error);
      }
    }
    try {
      supervisorTerminal = (
        await settleDirectChild(
          supervisor,
          outerDeadline,
          {
            normalMs: S2_FAULT_CLOSEOUT_MS.supervisorNormal,
            termMs: S2_FAULT_CLOSEOUT_MS.supervisorTerm,
            killMs: S2_FAULT_CLOSEOUT_MS.supervisorKill,
            label: "s2-custody-supervisor-fault-closeout",
            allowSignal: true,
          },
        )
      ).terminal;
    } catch (error) {
      secondary.push(error);
    }
  }
  if (retainedSupervisorFrames.length > 0) {
    secondary.push(
      new HostAuthorityError(
        "host.s2_fault_closeout_evidence",
        "fault closeout retained supervisor result frames",
        Object.freeze({
          frames: Object.freeze(retainedSupervisorFrames),
        }),
      ),
    );
  }
  return Object.freeze({
    workerTerminal,
    supervisorTerminal,
    retainedSupervisorFrames: Object.freeze(
      retainedSupervisorFrames,
    ),
    secondary: Object.freeze(secondary),
  });
}

function selectS2ConcurrentOutcomes(
  relayOutcome,
  workerOutcome,
) {
  requireCondition(
    relayOutcome?.role === "relay" &&
      workerOutcome?.role === "worker" &&
      (relayOutcome.kind === "VALUE" ||
        relayOutcome.kind === "FAULT") &&
      (workerOutcome.kind === "VALUE" ||
        workerOutcome.kind === "FAULT") &&
      Number.isInteger(relayOutcome.ordinal) &&
      Number.isInteger(workerOutcome.ordinal) &&
      relayOutcome.ordinal >= 0 &&
      workerOutcome.ordinal >= 0 &&
      relayOutcome.ordinal <= 1 &&
      workerOutcome.ordinal <= 1 &&
      relayOutcome.ordinal !== workerOutcome.ordinal,
    "host.s2_relay",
    "S2 relay/worker outcome shape differs",
  );
  if (
    relayOutcome.kind === "VALUE" &&
    workerOutcome.kind === "VALUE"
  ) {
    return Object.freeze({
      relay: relayOutcome.value,
      worker: workerOutcome.value,
    });
  }
  const bothFaulted =
    relayOutcome.kind === "FAULT" &&
    workerOutcome.kind === "FAULT";
  const firstOutcome = bothFaulted
    ? relayOutcome.ordinal < workerOutcome.ordinal
      ? relayOutcome
      : workerOutcome
    : relayOutcome.kind === "FAULT"
      ? relayOutcome
      : workerOutcome;
  const secondaryOutcome = bothFaulted
    ? firstOutcome === relayOutcome
      ? workerOutcome
      : relayOutcome
    : undefined;
  const first = firstOutcome.error;
  const secondary =
    secondaryOutcome === undefined
      ? []
      : [secondaryOutcome.error];
  const inheritedSecondaries = Array.isArray(
    first?.data?.secondaryFaults,
  )
    ? first.data.secondaryFaults
    : [];
  throw new HostAuthorityError(
    typeof first?.code === "string"
      ? first.code
      : "host.s2_relay",
    String(
      first?.message ??
        "S2 relay/worker concurrent outcome faulted",
    ),
    {
      firstFault:
        first?.data?.firstFault ?? safeError(first),
      secondaryFaults: Object.freeze([
        ...inheritedSecondaries,
        ...secondary.map(safeError),
      ]),
      retained: true,
    },
  );
}

async function runS2Supervisor(outerDeadline) {
  requireCondition(
    process.execPath === NODE &&
      process.argv0 === NODE &&
      process.argv.length === 2 &&
      outerDeadline instanceof AbsoluteDeadline,
    "host.supervisor_runtime",
    "S2 supervisor invocation/runtime differs",
    {
      execPath: process.execPath,
      argv0: process.argv0,
      argvCount: process.argv.length,
    },
  );
  const outerDeadlineNs = outerDeadline.endsNs;
  const startNs =
    outerDeadlineNs - 660_000_000_000n;
  const initialStatic = staticPass(
    outerDeadline.sub(
      "initial-static",
      DEADLINE_MS.initialStatic,
    ),
  );
  const setupDeadline = outerDeadline.sub(
    "setup",
    DEADLINE_MS.supervisorSetup,
  );
  const runToken = randomToken();
  const state = createInvocation(
    runToken,
    setupDeadline,
    new CapacityLedger(),
  );
  const evidenceFile = createEvidenceFile(
    state,
    setupDeadline,
  );
  evidenceFile.writer.add(
    "static",
    "supervisor.initial_static",
    {
      static: initialStatic,
      startNs: startNs.toString(),
      outerDeadlineNs: outerDeadlineNs.toString(),
    },
    setupDeadline,
  );
  evidenceFile.writer.add(
    "envelope",
    "supervisor.setup",
    {
      version: S2_PACKET.version,
      packet: S2_PACKET,
      runToken,
      invocation: {
        pathHash: state.invocationReceipt.pathHash,
        dev: state.invocationReceipt.dev,
        ino: state.invocationReceipt.ino,
      },
      evidence: {
        dev: evidenceFile.identity.dev,
        ino: evidenceFile.identity.ino,
        nlink: evidenceFile.identity.nlink,
        pathHash: sha256(
          Buffer.from(evidenceFile.path),
        ),
      },
    },
    setupDeadline,
  );
  evidenceFile.writer.finish(setupDeadline);
  const preSpawnDeadline = outerDeadline.sub(
    "pre-spawn",
    DEADLINE_MS.preSpawnRecheck,
  );
  const nodeRecheck = streamRegular(
    NODE,
    FIXED_PINS[0],
    preSpawnDeadline,
    initialStatic.files[0],
  );
  const ownerRecheck = streamOwner(
    preSpawnDeadline,
    initialStatic.owner,
  );
  const preSpawnNowNs = process.hrtime.bigint();
  requireCondition(
    outerDeadlineNs - preSpawnNowNs >=
      445_000_000_000n,
    "host.worker_deadline",
    "S2 supervisor lacks its pre-spawn outer reserve",
    {
      remainingNs: (
        outerDeadlineNs - preSpawnNowNs
      ).toString(),
      requiredNs: "445000000000",
    },
  );
  const workerDeadlineNs =
    preSpawnNowNs + 420_000_000_000n;
  const workerRemainingNs =
    workerDeadlineNs - preSpawnNowNs;
  const basisToken = randomToken();
  const initialBody = readBoundedRegular(
    evidenceFile.path,
    EVIDENCE_MAX_BYTES,
    preSpawnDeadline,
    evidenceFile.identity,
  );
  const initialParsed = parseEvidence(initialBody.body);
  const initialAppend = openEvidenceAppend(
    evidenceFile.path,
    evidenceFile.identity,
    initialParsed,
    preSpawnDeadline,
  );
  initialAppend.add(
    "static",
    "supervisor.pre_spawn",
    {
      node: nodeRecheck,
      owner: ownerRecheck,
      basisToken,
      preSpawnNowNs: preSpawnNowNs.toString(),
      outerDeadlineNs: outerDeadlineNs.toString(),
      workerDeadlineNs: workerDeadlineNs.toString(),
      workerRemainingNs: workerRemainingNs.toString(),
      protocol: S2_PROTOCOL,
      packet: S2_PACKET,
    },
    preSpawnDeadline,
  );
  const initialSnapshot =
    initialAppend.finish(preSpawnDeadline);
  const tombstones = new Tombstones();
  requireS2FaultCloseoutReserve(
    S2_FAULT_CLOSEOUT_MS.reserve,
  );
  const operationDeadline = outerDeadline.atMost(
    "s2-operation",
    outerDeadline.endsNs -
      BigInt(S2_FAULT_CLOSEOUT_MS.reserve) * 1_000_000n,
  );
  const custody = new S2OuterCustody(outerDeadline);
  return custody.run(async (custodyOwner) => {
  const {
    reservations: protocolReservations,
    supervisorNull,
  } = openS2ProtocolBootstrap(
    state,
    custodyOwner,
    preSpawnDeadline,
  );
  requireProtocolPeak(
    8,
    "supervisor spawned before parent close",
  );
  const supervisor = spawnExact({
      executable: RUBY,
      args: [
        "--disable=gems,rubyopt,did_you_mean",
        "-e",
        CUSTODY_SUPERVISOR_LITERAL,
        "--",
        state.invocation,
        outerDeadlineNs.toString(),
        S2_TARGET_LITERAL,
        sha256(Buffer.from(S2_TARGET_LITERAL)),
      ],
      cwd: state.invocation,
      env: closedEnvironment(
        state.invocation,
        state.invocation,
      ),
      stdio: ["pipe", "pipe", supervisorNull.fd],
      label: "s2-ruby-custody-supervisor",
      tombstones,
      onSpawn: (provisional) => {
        custodyOwner.bindSupervisor(provisional);
      },
    });
  custodyOwner.bindSupervisor(supervisor);
  custodyOwner.closeDescriptors(
    [supervisorNull],
    preSpawnDeadline,
  );
  requireCondition(
    supervisor.child.stdin !== null &&
      supervisor.child.stdout !== null,
    "host.s2_protocol",
    "custody supervisor control endpoints are absent",
  );
  const supervisorReader = new S2FrameReader(
    supervisor.child.stdout,
    "custody supervisor result",
    new S2TransportBudget(
      "custody supervisor result",
      S2_PROTOCOL.supervisorFaultOutputFrames,
      (S2_PROTOCOL.supervisorFaultOutputFrames *
        S2_PROTOCOL.frameBytes),
    ),
    operationDeadline,
  );
  custodyOwner.bindSupervisor(supervisor, supervisorReader);
  let readyRaw;
  readyRaw = await supervisorReader.read();
  requireCondition(
    readyRaw !== null,
    "host.s2_protocol",
    "custody supervisor ended before READY",
  );
  const bootstrapStop = parseS2SupervisorStop(readyRaw);
  requireCondition(
    bootstrapStop === null,
    "host.s2_bootstrap_stop",
    "custody supervisor selected bootstrap STOP",
    {
      stop: bootstrapStop,
      raw: s2RawFact(readyRaw),
    },
  );
  s2ParseReady(readyRaw);
  const workerNulls = openS2NullSet(
    custodyOwner,
    [
      fsConstants.O_RDONLY,
      fsConstants.O_WRONLY,
      fsConstants.O_WRONLY,
    ],
    preSpawnDeadline,
  );
  requireProtocolPeak(
    8,
    "worker spawned before parent close",
  );
  const worker = spawnExact({
      executable: NODE,
      args: [
        OWNER_PATH,
        "--vsq-s2-worker",
        runToken,
        state.invocation,
        state.invocationReceipt.dev,
        state.invocationReceipt.ino,
        evidenceFile.identity.dev,
        evidenceFile.identity.ino,
        `chain:${initialSnapshot.previousHash}`,
        outerDeadlineNs.toString(),
        workerDeadlineNs.toString(),
        workerRemainingNs.toString(),
        `basis:${basisToken}`,
      ],
      cwd: state.invocation,
      env: closedEnvironment(
        state.invocation,
        state.invocation,
      ),
      stdio: [
        workerNulls[0].fd,
        workerNulls[1].fd,
        workerNulls[2].fd,
        "pipe",
      ],
      label: "phase-a0-s2-worker",
      tombstones,
      onSpawn: (provisional) => {
        custodyOwner.bindWorker(provisional);
      },
    });
  custodyOwner.bindWorker(worker);
  custodyOwner.closeDescriptors(
    workerNulls,
    preSpawnDeadline,
  );
  requireCondition(
    worker.child.stdio[3] !== null,
    "host.s2_protocol",
    "worker relay endpoint is absent",
  );
  completeS2ProtocolReservations(
    state.capacity,
    protocolReservations,
  );
  let relayResult;
  let workerSettled;
  const concurrentSettlement =
    new S2ConcurrentSettlementLatch();
  const [relayOutcome, workerOutcome] = await Promise.all([
      concurrentSettlement.capture(
        "relay",
        runS2RelayServer({
          supervisor,
          supervisorReader,
          readyRaw,
          workerStream: worker.child.stdio[3],
          worker,
          deadline: operationDeadline,
        }),
      ),
      concurrentSettlement.capture(
        "worker",
        waitForWorker(
          worker,
          workerDeadlineNs,
          operationDeadline,
        ),
      ),
    ]);
  const selectedOutcomes = selectS2ConcurrentOutcomes(
    relayOutcome,
    workerOutcome,
  );
  relayResult = selectedOutcomes.relay;
  workerSettled = selectedOutcomes.worker;
  if (relayResult.nodeStop !== undefined) {
    throw new HostAuthorityError(
      "host.s2_node_stop",
      "custody supervisor selected the retained Node STOP route",
      {
        nodeStop: relayResult.nodeStop,
        rawNodeStop: s2RawFact(
          relayResult.rawNodeStop,
        ),
        supervisorTerminal:
          relayResult.supervisorTerminal,
        secondary: relayResult.secondary,
        workerRelayEof: relayResult.workerRelayEof,
        relayCloseoutFault:
          relayResult.relayCloseoutFault,
        workerTerminal: workerSettled.terminal,
        retained: true,
      },
    );
  }
  let supervisorTerminal;
  requireCondition(
    !workerSettled.forced,
    "WORKER_FORCED_REAP_STOP",
    "S2 worker required forced retirement",
    workerSettled,
  );
  requireExactTerminal(
    workerSettled.terminal,
    "host.worker_terminal",
    "phase-a0-s2-worker",
  );
  supervisorTerminal = await waitFor(
      operationDeadline.sub(
        "custody-supervisor-terminal",
        DEADLINE_MS.supervisorPostWorker,
      ),
      supervisor.terminal.promise,
      "SUPERVISOR_TERMINAL_STOP",
      "custody supervisor did not reap after clean close",
  );
  requireExactTerminal(
    supervisorTerminal,
    "host.s2_supervisor_terminal",
    "s2-ruby-custody-supervisor",
  );
  tombstones.add(worker.pid, "S2_WORKER_DIRECT_REAP");
  tombstones.add(
    supervisor.pid,
    "S2_CUSTODY_SUPERVISOR_DIRECT_REAP",
  );
  const postDeadline = operationDeadline.sub(
    "post-worker",
    DEADLINE_MS.supervisorPostWorker,
  );
  const finalEvidence = readBoundedRegular(
    evidenceFile.path,
    EVIDENCE_MAX_BYTES,
    postDeadline,
    evidenceFile.identity,
  );
  const parsed = parseEvidence(finalEvidence.body);
  const workerFinal = findEvidenceRecord(
    parsed,
    "worker.final",
  );
  const closeout = parsed.records.at(-1);
  requireCondition(
    parsed.records.length ===
        S2_PROTOCOL.cleanEvidenceRecords &&
      closeout.kind === "supervisor.lifecycle" &&
      closeout.facts.lifecycle === "CLOSEOUT" &&
      relayResult.finalAck.evidenceSequence ===
        closeout.sequence &&
      relayResult.finalAck.evidenceHash === closeout.hash &&
      sameRoot(
        state.invocation,
        state.invocationReceipt,
      ) &&
      sameRoot(
        state.evidenceRoot,
        state.evidenceRootReceipt,
      ) &&
      canonicalJson(readdirSync(state.invocation)) ===
        canonicalJson(["evidence"]) &&
      canonicalJson(readdirSync(state.evidenceRoot)) ===
        canonicalJson(["a0.jsonl"]),
    "host.closeout",
    "S2 supervisor final evidence/root relation differs",
    {
      records: parsed.records.length,
      closeout: closeout.kind,
      finalAck: relayResult.finalAck,
    },
  );
  const semantic = validateS2Evidence(
    parsed,
    Object.freeze({
      finalAck: relayResult.finalAck,
      rawFinalAck: relayResult.rawFinalAck,
      supervisorTerminal,
      workerTerminal: workerSettled.terminal,
      supervisorResultEof: true,
      workerRelayEof: true,
      retainedInvocation: state.invocation,
      invocationReceipt: state.invocationReceipt,
      evidenceRootReceipt: state.evidenceRootReceipt,
      evidenceIdentity: finalEvidence.identity,
    }),
  );
  emitResult({
    code: "host.phase_a0_provisional_clean",
    outcome: "PROVISIONAL_PHASE_A0_CLEAN",
    releases: "CORE_REPLAY_AMENDMENT_ONLY",
    sourceSha256: initialStatic.owner.sha256,
    packet: S2_PACKET,
    evidence: {
      pathHash: sha256(Buffer.from(evidenceFile.path)),
      dev: finalEvidence.identity.dev,
      ino: finalEvidence.identity.ino,
      nlink: finalEvidence.identity.nlink,
      bytes: finalEvidence.bytes,
      sha256: finalEvidence.sha256,
      chain: parsed.previousHash,
      records: parsed.records.length,
    },
    relay: {
      finalAck: s2RawFact(relayResult.rawFinalAck),
      finalAckValue: relayResult.finalAck,
      rolePids: Object.freeze({
        controller: process.pid,
        custodySupervisor: supervisor.pid,
        worker: worker.pid,
      }),
      supervisorResultEof: true,
      workerRelayEof: true,
      supervisorTerminal,
      workerTerminal: workerSettled.terminal,
    },
    counters: workerFinal.facts.counters,
    semantic,
    retainedInvocation: state.invocation,
    retainedRoot: {
      pathHash: state.invocationReceipt.pathHash,
      dev: state.invocationReceipt.dev,
      ino: state.invocationReceipt.ino,
      uid: state.invocationReceipt.uid,
      gid: state.invocationReceipt.gid,
      mode: state.invocationReceipt.mode,
    },
    retainedEvidenceRoot: {
      pathHash: state.evidenceRootReceipt.pathHash,
      dev: state.evidenceRootReceipt.dev,
      ino: state.evidenceRootReceipt.ino,
      uid: state.evidenceRootReceipt.uid,
      gid: state.evidenceRootReceipt.gid,
      mode: state.evidenceRootReceipt.mode,
    },
  }, outerDeadline);
  return 0;
  });
}

function rejects(callback, label) {
  let rejected = false;
  try {
    callback();
  } catch {
    rejected = true;
  }
  requireCondition(
    rejected,
    "host.self_test",
    `${label} did not reject`,
  );
}

async function rejectsAsync(callback, label) {
  let rejected = false;
  try {
    await callback();
  } catch {
    rejected = true;
  }
  requireCondition(
    rejected,
    "host.self_test",
    `${label} did not reject`,
  );
}

function runPsParserSelfTests() {
  const atFailure = Buffer.from(
    "NjYzODUgNjYzNzkgNjYzODUgICAgICAwICAgNTAxIFRzICAgRnJpIEp1bCAyNCAwMzoxODo1MiAyMDI2ICAgICBydWJ5ICAgICAgICAgICAgCg==",
    "base64",
  );
  const postCapture = Buffer.from(
    "NjYzODUgICAgIDEgNjYzODUgICAgICAwICAgNTAxIFRzICAgRnJpIEp1bCAyNCAwMzoxODo1MiAyMDI2ICAgICBydWJ5ICAgICAgICAgICAgCjY2Mzg3IDY2Mzg1IDY2Mzg1ICAgICAgMCAgIDUwMSBUICAgIEZyaSBKdWwgMjQgMDM6MTg6NTIgMjAyNiAgICAgcnVieSAgICAgICAgICAgIAo=",
    "base64",
  );
  requireCondition(
    atFailure.length === 82 &&
      sha256(atFailure.subarray(0, -1)) ===
        "ca2b360e6aecbe8896feffe055ad6deda5e9baa806a857b932e7d003dd4f5eb6" &&
      postCapture.length === 164 &&
      sha256(postCapture) ===
        "15feebde81efa71b0d7bf81163512f6fe8c46d2841031cfcf87c728e70d71189",
    "host.self_test",
    "exact ps fixture identity differs",
  );
  const atFailureRows = parsePsRows(atFailure);
  const postRows = parsePsRows(postCapture);
  requireCondition(
    atFailureRows.length === 1 &&
      atFailureRows[0].pid === 66_385 &&
      atFailureRows[0].ppid === 66_379 &&
      atFailureRows[0].pgid === 66_385 &&
      atFailureRows[0].sessObservedZero === "0" &&
      atFailureRows[0].state === "Ts" &&
      postRows.length === 2 &&
      postRows[0].pid === 66_385 &&
      postRows[0].ppid === 1 &&
      postRows[1].pid === 66_387 &&
      postRows[1].ppid === 66_385 &&
      postRows.every(
        (row) =>
          row.sessObservedZero === "0" &&
          row.ucomm === "ruby",
      ),
    "host.self_test",
    "exact ps fixture projection differs",
  );
  const psFact = stableProcessFact(atFailureRows[0]);
  requireCondition(
    Object.hasOwn(psFact, "sessObservedZero") &&
      !Object.hasOwn(psFact, "sid") &&
      !Object.hasOwn(psFact, "receiptDigest"),
    "host.self_test",
    "ps fact improperly carries receipt SID/digest",
  );

  const atFailureText = atFailure
    .subarray(0, -1)
    .toString("ascii");
  const sessNeedle = "0   501";
  const sessOffset = atFailureText.indexOf(sessNeedle);
  requireCondition(
    sessOffset >= 0,
    "host.self_test",
    "ps sess KAT offset differs",
  );
  for (const sess of ["1", "00", "01", "66385"]) {
    const candidate = Buffer.from(
      `${atFailureText.slice(0, sessOffset)}${sess}${atFailureText.slice(
        sessOffset + 1,
      )}\n`,
      "ascii",
    );
    rejects(() => parsePsRows(candidate), `ps sess ${sess}`);
  }
  const ucommNeedle = "ruby            ";
  const ucommOffset = atFailureText.indexOf(ucommNeedle);
  requireCondition(
    ucommOffset >= 0,
    "host.self_test",
    "ps ucomm KAT offset differs",
  );
  for (const padding of [11, 13]) {
    rejects(
      () =>
        parsePsRows(
          Buffer.from(
            `${atFailureText.slice(0, ucommOffset)}ruby${" ".repeat(
              padding,
            )}\n`,
            "ascii",
          ),
        ),
      `ps ucomm padding ${padding}`,
    );
  }
  for (const mutatedText of [
    atFailureText.replace("ruby            ", "ruby\t           "),
    atFailureText.replace("ruby            ", "rubyX           "),
    atFailureText.replace("0   501", "+0   501"),
    atFailureText.replace("0   501", "00   501"),
    atFailureText.replace("66385", "066385"),
  ]) {
    rejects(
      () => parsePsRows(Buffer.from(`${mutatedText}\n`, "ascii")),
      "ps exact row spelling",
    );
  }
  for (const mutate of [
    (bytes) => {
      bytes[0] = 0xb6;
    },
    (bytes) => {
      const offset = bytes.indexOf(Buffer.from("0   501", "ascii"));
      bytes[offset] = 0xb0;
    },
    (bytes) => {
      const offset = bytes.indexOf(
        Buffer.from("ruby            ", "ascii"),
      );
      bytes[offset] = 0xf2;
    },
    (bytes) => {
      const offset = bytes.indexOf(
        Buffer.from("ruby            ", "ascii"),
      );
      bytes[offset + 4] = 0xa0;
    },
  ]) {
    const bytes = Buffer.from(atFailure);
    mutate(bytes);
    rejects(() => parsePsRows(bytes), "ps high-bit alias");
  }
  for (const malformed of [
    atFailure.subarray(0, -1),
    Buffer.concat([atFailure, Buffer.from("\n")]),
    Buffer.concat([atFailure.subarray(0, -1), Buffer.from("\r\n")]),
    Buffer.concat([atFailure.subarray(0, -1), Buffer.from([0, 0x0a])]),
    Buffer.concat([atFailure, atFailure]),
    Buffer.concat([
      postCapture.subarray(82),
      postCapture.subarray(0, 82),
    ]),
    Buffer.concat([postCapture, atFailure]),
  ]) {
    rejects(() => parsePsRows(malformed), "ps framing/order/count");
  }
  return Object.freeze({
    atFailureBytes: atFailure.length,
    postCaptureBytes: postCapture.length,
    exactRows: atFailureRows.length + postRows.length,
    sess: "RubyPsSessObservedZero",
    ucommPadding: 12,
  });
}

function runRubyTopologySelfTests() {
  const launchPid = 123;
  const parentReceipt = Object.freeze({
    role: "P",
    branch: "S",
    pid: launchPid,
    ppid: process.pid,
    pgid: launchPid,
    sid: launchPid,
    dev: "1",
    ino: "2",
  });
  const childReceipt = Object.freeze({
    role: "C",
    branch: "S",
    pid: 124,
    ppid: launchPid,
    pgid: launchPid,
    sid: launchPid,
    dev: "1",
    ino: "3",
  });
  const parentRow = Object.freeze({
    pid: launchPid,
    ppid: process.pid,
    pgid: launchPid,
    sessObservedZero: "0",
    uid: HOST_UID,
    state: "T",
    lstart: "Fri Jul 24 03:18:52 2026",
    ucomm: "ruby",
  });
  const childRow = Object.freeze({
    pid: 124,
    ppid: launchPid,
    pgid: launchPid,
    sessObservedZero: "0",
    uid: HOST_UID,
    state: "T",
    lstart: "Fri Jul 24 03:18:52 2026",
    ucomm: "ruby",
  });
  const childReparented = Object.freeze({
    ...childRow,
    ppid: 1,
  });
  const makeState = (
    receipts,
    intentionalOneReceipt,
    receiptClassifiedRoute,
    receiptProofRoute,
  ) => {
    const state = {
      leg: "kat",
      launchPid,
      intentionalOneReceipt,
      receiptClassifiedRoute,
      receiptProofRoute,
      receiptByPid: new Map(),
      roles: new Map(),
      roleFactsByPid: new Map(),
      provisionalByPid: new Map(),
      cleanupByPid: new Map(),
      frozen: new Map(),
      parentReaped: false,
      reconcile05: undefined,
      promotedReparent: undefined,
      confirmedReparent: undefined,
      scope: undefined,
    };
    for (const receipt of receipts) {
      state.receiptByPid.set(receipt.pid, receipt);
      state.roles.set(receipt.role, receipt.pid);
    }
    state.scope = issueRubyScope(launchPid, receipts);
    return state;
  };
  const proofOwner = new RubyProofOwner(
    {},
    {},
    {},
    { has() { return false; } },
    {},
  );
  let provisionalOrders = 0;
  let typedCounterpartTargets = 0;
  for (const receipt of [parentReceipt, childReceipt]) {
    const state = makeState(
      [receipt],
      true,
      "PARSER_INTENTIONALLY_STOPPED_AFTER_ONE_RECORD",
      "EXISTING_COMPLEX_ONE_RECEIPT_BRANCH",
    );
    proofOwner.bindRows(state, [parentRow, childRow], 1);
    requireCondition(
      state.provisionalByPid.size === 1 &&
        state.cleanupByPid.size === 0 &&
        state.roleFactsByPid.size === 2 &&
        [...state.provisionalByPid.values()].every(
          (fact) =>
            fact.kind === "OneReceiptProvisionalRubyRole" &&
            !Object.hasOwn(fact, "sid") &&
            !Object.hasOwn(fact, "receiptDigest"),
        ) &&
        [...state.roleFactsByPid.values()]
          .filter(
            (fact) => fact.kind === "ReceiptBoundRubyRoleFact",
          )
          .every(
            (fact) =>
              Object.hasOwn(fact, "sid") &&
              Object.hasOwn(fact, "receiptDigest"),
          ),
      "host.self_test",
      "intentional provisional Ruby role projection differs",
    );
    const provisionalBefore = [
      ...state.provisionalByPid.values(),
    ][0];
    proofOwner.bindRows(state, [parentRow, childRow], 2);
    requireCondition(
      [...state.provisionalByPid.values()][0] ===
        provisionalBefore,
      "host.self_test",
      "later capture replaced the provisional Ruby role",
    );
    requireCondition(
      issuedRubyPositiveTargets.has(provisionalBefore) &&
        provisionalBefore.kind ===
          "OneReceiptProvisionalRubyRole" &&
        !Object.hasOwn(provisionalBefore, "rawSignalSelector"),
      "host.self_test",
      "intentional provisional observation fact differs",
    );
    typedCounterpartTargets += 1;
    provisionalOrders += 1;
  }

  let cleanupOrders = 0;
  for (const route of CLEANUP_COUNTERPART_ROUTES) {
    for (const receipt of [parentReceipt, childReceipt]) {
      const state = makeState(
        [receipt],
        false,
        route,
        "RECEIPT_ANCHORED_EARLY_CLEANUP",
      );
      proofOwner.bindRows(state, [parentRow, childRow], 12);
      requireCondition(
        state.cleanupByPid.size === 1 &&
          state.provisionalByPid.size === 0 &&
          [...state.cleanupByPid.values()].every(
            (fact) =>
              fact.kind === "CleanupBoundRubyCounterpart" &&
              !Object.hasOwn(fact, "sid") &&
              !Object.hasOwn(fact, "receiptDigest"),
          ),
        "host.self_test",
        "cleanup-only Ruby counterpart projection differs",
      );
      const cleanupTarget = [
        ...state.cleanupByPid.values(),
      ][0];
      requireCondition(
        issuedRubyPositiveTargets.has(cleanupTarget) &&
          cleanupTarget.kind ===
            "CleanupBoundRubyCounterpart" &&
          !Object.hasOwn(cleanupTarget, "rawSignalSelector"),
        "host.self_test",
        "cleanup counterpart observation fact differs",
      );
      typedCounterpartTargets += 1;
      cleanupOrders += 1;
    }
  }

  const phaseState = makeState(
    [childReceipt],
    true,
    "PARSER_INTENTIONALLY_STOPPED_AFTER_ONE_RECORD",
    "EXISTING_COMPLEX_ONE_RECEIPT_BRANCH",
  );
  proofOwner.bindRows(phaseState, [parentRow, childRow], 1);
  phaseState.parentReaped = true;
  proofOwner.bindRows(phaseState, [childReparented], 5);
  requireCondition(
    phaseState.reconcile05?.kind ===
      "PostParentReapReconcile05" &&
      phaseState.promotedReparent === undefined &&
      phaseState.confirmedReparent === undefined,
    "host.self_test",
    "ordinal 05 minted a reparent fact",
  );
  proofOwner.bindRows(phaseState, [childReparented], 6);
  requireCondition(
    phaseState.promotedReparent?.kind ===
      "PromotedRubyReparentFact" &&
      phaseState.confirmedReparent === undefined,
    "host.self_test",
    "ordinal 06 did not uniquely promote the reparent fact",
  );
  proofOwner.bindRows(phaseState, [childReparented], 7);
  proofOwner.bindRows(phaseState, [childReparented], 8);
  requireCondition(
    phaseState.confirmedReparent?.kind ===
      "ConfirmedRubyReparentFact",
    "host.self_test",
    "ordinal 07 did not confirm the promoted reparent fact",
  );
  rejects(
    () =>
      proofOwner.bindRows(
        phaseState,
        [parentRow, childReparented],
        9,
      ),
    "post-reap parent reappearance",
  );
  const lateAcquire = makeState(
    [parentReceipt],
    false,
    "ORDINARY_ONE_RECORD_EOF",
    "RECEIPT_ANCHORED_EARLY_CLEANUP",
  );
  lateAcquire.parentReaped = true;
  rejects(
    () =>
      proofOwner.bindRows(
        lateAcquire,
        [parentRow, childReparented],
        12,
      ),
    "post-reap counterpart acquisition",
  );

  const preparedReparentState = () => {
    const state = makeState(
      [childReceipt],
      true,
      "PARSER_INTENTIONALLY_STOPPED_AFTER_ONE_RECORD",
      "EXISTING_COMPLEX_ONE_RECEIPT_BRANCH",
    );
    proofOwner.bindRows(state, [parentRow, childRow], 1);
    state.parentReaped = true;
    return state;
  };
  let negativeReparentCases = 0;
  const nonOneAt05 = preparedReparentState();
  rejects(
    () => proofOwner.bindRows(nonOneAt05, [childRow], 5),
    "ordinal 05 non-one PPID",
  );
  negativeReparentCases += 1;

  const noReconcile = preparedReparentState();
  rejects(
    () =>
      proofOwner.bindRows(noReconcile, [childReparented], 6),
    "ordinal 06 promotion without reconciliation",
  );
  negativeReparentCases += 1;

  const secondPromotion = preparedReparentState();
  proofOwner.bindRows(secondPromotion, [childReparented], 5);
  proofOwner.bindRows(secondPromotion, [childReparented], 6);
  rejects(
    () =>
      proofOwner.bindRows(
        secondPromotion,
        [childReparented],
        6,
      ),
    "ordinal 06 second promotion",
  );
  negativeReparentCases += 1;

  const mismatchAt07 = preparedReparentState();
  proofOwner.bindRows(mismatchAt07, [childReparented], 5);
  proofOwner.bindRows(mismatchAt07, [childReparented], 6);
  rejects(
    () =>
      proofOwner.bindRows(
        mismatchAt07,
        [
          Object.freeze({
            ...childReparented,
            lstart: "Fri Jul 24 03:18:53 2026",
          }),
        ],
        7,
      ),
    "ordinal 07 identity mismatch",
  );
  negativeReparentCases += 1;

  const reconcileOnly = preparedReparentState();
  proofOwner.bindRows(reconcileOnly, [childReparented], 5);
  rejects(
    () =>
      proofOwner.bindRows(
        reconcileOnly,
        [childReparented],
        8,
      ),
    "ordinal 05 observation used without promotion",
  );
  negativeReparentCases += 1;

  const secondChild = preparedReparentState();
  proofOwner.bindRows(secondChild, [childReparented], 5);
  proofOwner.bindRows(secondChild, [childReparented], 6);
  proofOwner.bindRows(secondChild, [childReparented], 7);
  rejects(
    () =>
      proofOwner.bindRows(
        secondChild,
        [
          childReparented,
          Object.freeze({
            ...childReparented,
            pid: 125,
          }),
        ],
        8,
      ),
    "post-reap second child",
  );
  negativeReparentCases += 1;

  const laterIdentityChange = preparedReparentState();
  proofOwner.bindRows(
    laterIdentityChange,
    [childReparented],
    5,
  );
  proofOwner.bindRows(
    laterIdentityChange,
    [childReparented],
    6,
  );
  proofOwner.bindRows(
    laterIdentityChange,
    [childReparented],
    7,
  );
  rejects(
    () =>
      proofOwner.bindRows(
        laterIdentityChange,
        [
          Object.freeze({
            ...childReparented,
            state: "S",
            lstart: "Fri Jul 24 03:18:53 2026",
          }),
        ],
        8,
      ),
    "post-reap stable identity change",
  );
  negativeReparentCases += 1;

  const signalState = makeState(
    [parentReceipt, childReceipt],
    false,
    "ORDINARY_SUCCESS",
    "support",
  );
  proofOwner.bindRows(signalState, [parentRow, childRow], 1);
  const positive =
    signalState.roleFactsByPid.get(childRow.pid);
  const group = signalState.scope;
  requireCondition(
    issuedRubyPositiveTargets.has(positive) &&
      issuedRubyScopes.has(group) &&
      positive.kind ===
        "ReceiptBoundRubyRoleFact" &&
      group.kind === "ReceiptAnchoredRubyScope" &&
      !Object.hasOwn(positive, "rawSignalSelector") &&
      !Object.hasOwn(group, "rawSignalSelector"),
    "host.self_test",
    "typed Ruby observation facts differ",
  );
  requireCondition(
    provisionalOrders === 2 &&
      cleanupOrders === 10 &&
      typedCounterpartTargets === 12 &&
      negativeReparentCases === 7,
    "host.self_test",
    "Ruby topology recurrence matrix count differs",
  );

  return Object.freeze({
    provisionalOrders,
    cleanupOrders,
    typedCounterpartTargets,
    reparentPhases: 3,
    negativeReparentCases,
    rawPidSignalAuthority: false,
    numericSelectorPresent: false,
  });
}

function receiptKatHash(length, byte = 0x62) {
  requireCondition(
    Number.isSafeInteger(length) &&
      length >= 1 &&
      length <= 192,
    "host.self_test",
    "receipt KAT hash length differs",
  );
  return sha256(Buffer.alloc(length, byte));
}

function receiptKatSource(
  prefix,
  trailingLength = 0,
  operationLeg = "SUPPORT",
) {
  const index = RECEIPT_PREFIX_NAMES.indexOf(prefix);
  requireCondition(
    index >= 0,
    "host.self_test",
    "receipt KAT prefix differs",
  );
  return {
    roles: [...RECEIPT_PREFIXES[index]],
    trailingLength,
    trailingSha256:
      trailingLength === 0
        ? NO_TRAILING_SHA256
        : receiptKatHash(trailingLength),
    operationLeg,
  };
}

function requireReceiptKatTerminal(
  step,
  family,
  terminal,
  label,
) {
  requireCondition(
    step.latch !== null &&
      step.latch.family === family &&
      step.latch.terminal === terminal &&
      step.nextSourceState === null,
    "host.self_test",
    `${label} terminal differs`,
  );
}

function runReceiptTransportSelfTests() {
  requireCondition(
    exactPrimitiveArray(RECEIPT_SOURCE_FIELDS, [
      "roles",
      "trailingLength",
      "trailingSha256",
      "operationLeg",
    ]) &&
      exactPrimitiveArray(RECEIPT_PAYLOAD_FIELDS, [
        "family",
        "terminal",
        "observedPrefix",
        "authorityPrefix",
        "trailingLength",
        "trailingSha256",
        "selectedLeg",
        "classifiedRoute",
        "proofRoute",
        "outcome",
      ]) &&
      exactPrimitiveArray(RECEIPT_OPERATION_LEGS, [
        "SUPPORT",
        "DENIAL",
        "PARENT_LOSS",
        "ONE_RECEIPT_P_FIRST",
        "ONE_RECEIPT_C_FIRST",
      ]) &&
      exactPrimitiveArray(Object.keys(RECEIPT_TERMINAL_FAMILY), [
        "CAPACITY_RECORD_193",
        "GRAMMAR_MALFORMED_OR_DUPLICATE",
        "PROTOCOL_THIRD_RECORD",
        "PROTOCOL_INTENTIONAL_ONE_RECORD_STOP",
        "READ_NON_EAGAIN_ERROR",
        "EOF_TERMINAL",
        "DEADLINE_NO_EOF",
      ]),
    "host.self_test",
    "receipt closed domains differ",
  );

  const exactRoleSources = RECEIPT_PREFIX_NAMES.map((prefix) =>
    copyReceiptSourceState(receiptKatSource(prefix))
  );
  requireCondition(
    exactRoleSources.every(
      (source, index) =>
        source.observedPrefix === RECEIPT_PREFIX_NAMES[index] &&
        Object.isFrozen(source) &&
        Object.isFrozen(source.roles),
    ),
    "host.self_test",
    "receipt prefix snapshots differ",
  );
  for (const invalidSource of [
    {
      roles: ["P", "P"],
      trailingLength: 0,
      trailingSha256: NO_TRAILING_SHA256,
      operationLeg: "SUPPORT",
    },
    {
      roles: [],
      trailingLength: -0,
      trailingSha256: NO_TRAILING_SHA256,
      operationLeg: "SUPPORT",
    },
    {
      roles: [],
      trailingLength: 1,
      trailingSha256: NO_TRAILING_SHA256,
      operationLeg: "SUPPORT",
    },
    {
      roles: [],
      trailingLength: 0,
      trailingSha256: receiptKatHash(1),
      operationLeg: "SUPPORT",
    },
    {
      roles: [],
      trailingLength: 0,
      trailingSha256: NO_TRAILING_SHA256,
      operationLeg: "UNKNOWN",
    },
    {
      roles: [],
      trailingLength: 0,
      trailingSha256: NO_TRAILING_SHA256,
      operationLeg: "SUPPORT",
      alien: true,
    },
  ]) {
    rejects(
      () => copyReceiptSourceState(invalidSource),
      "invalid receipt source",
    );
  }
  const sparseRoles = [];
  sparseRoles.length = 1;
  rejects(
    () =>
      copyReceiptSourceState({
        roles: sparseRoles,
        trailingLength: 0,
        trailingSha256: NO_TRAILING_SHA256,
        operationLeg: "SUPPORT",
      }),
    "sparse receipt roles",
  );
  const symbolRoles = [];
  symbolRoles[Symbol("forged")] = "P";
  rejects(
    () =>
      copyReceiptSourceState({
        roles: symbolRoles,
        trailingLength: 0,
        trailingSha256: NO_TRAILING_SHA256,
        operationLeg: "SUPPORT",
      }),
    "symbol receipt roles",
  );
  let sourceGetterCalls = 0;
  const sourceWithGetter = {
    roles: [],
    trailingLength: 0,
    trailingSha256: NO_TRAILING_SHA256,
  };
  Object.defineProperty(sourceWithGetter, "operationLeg", {
    enumerable: true,
    get() {
      sourceGetterCalls += 1;
      return "SUPPORT";
    },
  });
  rejects(
    () => copyReceiptSourceState(sourceWithGetter),
    "receipt source getter",
  );
  requireCondition(
    sourceGetterCalls === 0,
    "host.self_test",
    "receipt source getter was invoked",
  );
  let proxyTrapCalls = 0;
  const sourceProxy = new Proxy(receiptKatSource("NONE"), {
    get() {
      proxyTrapCalls += 1;
      throw new Error("receipt source proxy get");
    },
    ownKeys() {
      proxyTrapCalls += 1;
      throw new Error("receipt source proxy ownKeys");
    },
    getPrototypeOf() {
      proxyTrapCalls += 1;
      throw new Error("receipt source proxy prototype");
    },
  });
  rejects(
    () => copyReceiptSourceState(sourceProxy),
    "receipt source proxy",
  );
  requireCondition(
    proxyTrapCalls === 0,
    "host.self_test",
    "receipt source proxy trap was invoked",
  );

  const equalityEof = receiptTransportStep({
    deadlineNs: 100n,
    preNowNs: 100n,
    postNowNs: 100n,
    readResult: "EOF",
  });
  requireReceiptKatTerminal(
    equalityEof,
    "EOF",
    "EOF_TERMINAL",
    "deadline equality EOF",
  );
  requireCondition(
    equalityEof.reads === 1 &&
      equalityEof.reservationAttempts === 1 &&
      equalityEof.reservationReleases === 1,
    "host.self_test",
    "deadline equality read accounting differs",
  );
  const preExpired = receiptTransportStep({
    deadlineNs: 100n,
    preNowNs: 101n,
    postNowNs: 101n,
    readResult: "EOF",
  });
  requireReceiptKatTerminal(
    preExpired,
    "DEADLINE",
    "DEADLINE_NO_EOF",
    "pre-read deadline",
  );
  requireCondition(
    preExpired.reads === 0 &&
      preExpired.reservationAttempts === 0,
    "host.self_test",
    "pre-read deadline accounting differs",
  );
  const postExpired = receiptTransportStep({
    deadlineNs: 100n,
    preNowNs: 99n,
    postNowNs: 101n,
    readResult: "POSITIVE_DATA",
    positiveByteKind: "LF",
    positiveTrailingSha256: receiptKatHash(1),
    lineParseResult: "VALID_P",
  });
  requireReceiptKatTerminal(
    postExpired,
    "DEADLINE",
    "DEADLINE_NO_EOF",
    "post-read deadline precedence",
  );
  requireCondition(
    postExpired.reads === 1 &&
      postExpired.committedBytes === 0 &&
      postExpired.reservationReleases === 1,
    "host.self_test",
    "post-read deadline accounting differs",
  );
  const equalityEagain = receiptTransportStep({
    deadlineNs: 100n,
    preNowNs: 100n,
    postNowNs: 100n,
    readResult: "EAGAIN",
  });
  requireCondition(
    equalityEagain.latch === null &&
      equalityEagain.yields === 1 &&
      equalityEagain.reads === 1 &&
      equalityEagain.reservationReleases === 1,
    "host.self_test",
    "EAGAIN equality accounting differs",
  );
  const afterEagain = receiptTransportStep({
    sourceState: equalityEagain.nextSourceState,
    deadlineNs: 100n,
    preNowNs: 101n,
    postNowNs: 101n,
    readResult: "EOF",
  });
  requireCondition(
    afterEagain.latch.terminal === "DEADLINE_NO_EOF" &&
      afterEagain.reads === 0,
    "host.self_test",
    "EAGAIN did not require a fresh deadline sample",
  );

  const deadlineEvents = Object.freeze([
    Object.freeze({
      name: "EOF",
      fields: Object.freeze({ readResult: "EOF" }),
      terminal: "EOF_TERMINAL",
    }),
    Object.freeze({
      name: "POSITIVE_DATA",
      fields: Object.freeze({
        readResult: "POSITIVE_DATA",
        positiveByteKind: "NON_LF",
        positiveTrailingSha256: receiptKatHash(1),
        lineParseResult: "NONE",
      }),
      terminal: null,
    }),
    Object.freeze({
      name: "EAGAIN",
      fields: Object.freeze({ readResult: "EAGAIN" }),
      terminal: null,
    }),
    Object.freeze({
      name: "NON_EAGAIN_READ_ERROR",
      fields: Object.freeze({
        readResult: "NON_EAGAIN_READ_ERROR",
      }),
      terminal: "READ_NON_EAGAIN_ERROR",
    }),
  ]);
  let deadlineMatrixCases = 0;
  for (const event of deadlineEvents) {
    for (const [offset, now] of [
      [-1, 99n],
      [0, 100n],
      [1, 101n],
    ]) {
      const step = receiptTransportStep({
        deadlineNs: 100n,
        preNowNs: now,
        postNowNs: now,
        ...event.fields,
      });
      if (offset === 1) {
        requireCondition(
          step.latch.terminal === "DEADLINE_NO_EOF" &&
            step.reads === 0 &&
            step.reservationAttempts === 0 &&
            step.committedBytes === 0,
          "host.self_test",
          `pre-read deadline ${event.name} +1 differs`,
        );
      } else if (event.name === "POSITIVE_DATA") {
        requireCondition(
          step.latch === null &&
            step.reads === 1 &&
            step.reservationAttempts === 1 &&
            step.reservationReleases === 0 &&
            step.committedBytes === 1,
          "host.self_test",
          `pre-read deadline ${event.name} ${offset} differs`,
        );
      } else if (event.name === "EAGAIN") {
        requireCondition(
          step.latch === null &&
            step.reads === 1 &&
            step.yields === 1 &&
            step.reservationReleases === 1,
          "host.self_test",
          `pre-read deadline ${event.name} ${offset} differs`,
        );
      } else {
        requireCondition(
          step.latch.terminal === event.terminal &&
            step.reads === 1 &&
            step.reservationReleases === 1 &&
            step.committedBytes === 0,
          "host.self_test",
          `pre-read deadline ${event.name} ${offset} differs`,
        );
      }
      deadlineMatrixCases += 1;
    }
    for (const [offset, postNowNs] of [
      [-1, 99n],
      [0, 100n],
      [1, 101n],
    ]) {
      const step = receiptTransportStep({
        deadlineNs: 100n,
        preNowNs: 99n,
        postNowNs,
        ...event.fields,
      });
      if (offset === 1) {
        requireCondition(
          step.latch.terminal === "DEADLINE_NO_EOF" &&
            step.reads === 1 &&
            step.reservationAttempts === 1 &&
            step.reservationReleases === 1 &&
            step.committedBytes === 0,
          "host.self_test",
          `post-return deadline ${event.name} +1 differs`,
        );
      } else if (event.name === "POSITIVE_DATA") {
        requireCondition(
          step.latch === null &&
            step.reads === 1 &&
            step.reservationReleases === 0 &&
            step.committedBytes === 1,
          "host.self_test",
          `post-return deadline ${event.name} ${offset} differs`,
        );
      } else if (event.name === "EAGAIN") {
        requireCondition(
          step.latch === null &&
            step.yields === 1 &&
            step.reservationReleases === 1,
          "host.self_test",
          `post-return deadline ${event.name} ${offset} differs`,
        );
      } else {
        requireCondition(
          step.latch.terminal === event.terminal &&
            step.reservationReleases === 1 &&
            step.committedBytes === 0,
          "host.self_test",
          `post-return deadline ${event.name} ${offset} differs`,
        );
      }
      deadlineMatrixCases += 1;
    }
  }
  requireCondition(
    deadlineMatrixCases === 24,
    "host.self_test",
    "receipt deadline matrix count differs",
  );

  const firstP = receiptTransportStep({
    sourceState: receiptKatSource("NONE", 69),
    readResult: "POSITIVE_DATA",
    positiveByteKind: "LF",
    positiveTrailingSha256: receiptKatHash(70),
    lineParseResult: "VALID_P",
  });
  requireCondition(
    firstP.latch === null &&
      firstP.nextSourceState.roles.length === 1 &&
      firstP.nextSourceState.roles[0] === "P" &&
      firstP.nextSourceState.trailingLength === 0,
    "host.self_test",
    "first valid receipt continuation differs",
  );
  const firstC = receiptTransportStep({
    sourceState: receiptKatSource("NONE", 69),
    readResult: "POSITIVE_DATA",
    positiveByteKind: "LF",
    positiveTrailingSha256: receiptKatHash(70),
    lineParseResult: "VALID_C",
  });
  requireCondition(
    firstC.latch === null &&
      firstC.nextSourceState.roles.length === 1 &&
      firstC.nextSourceState.roles[0] === "C" &&
      firstC.nextSourceState.trailingLength === 0,
    "host.self_test",
    "first valid C receipt continuation differs",
  );
  for (const invalid of [
    {
      readResult: "POSITIVE_DATA",
      positiveByteKind: "NON_LF",
      positiveTrailingSha256: receiptKatHash(1),
      lineParseResult: "VALID_P",
    },
    {
      readResult: "POSITIVE_DATA",
      positiveByteKind: "LF",
      positiveTrailingSha256: receiptKatHash(1),
      lineParseResult: "NONE",
    },
    {
      sourceState: receiptKatSource(
        "NONE",
        0,
        "ONE_RECEIPT_P_FIRST",
      ),
      readResult: "EOF",
      mode: "ORDINARY",
    },
    {
      sourceState: receiptKatSource("NONE"),
      readResult: "EOF",
      mode: "ONE_RECEIPT",
    },
  ]) {
    rejects(
      () => receiptTransportStep(invalid),
      "receipt line-result/mode cross-product",
    );
  }
  let ordinarySuccessCases = 0;
  for (const [firstPrefix, secondResult, wantedPrefix] of [
    ["P", "VALID_C", "P_C"],
    ["C", "VALID_P", "C_P"],
  ]) {
    for (const operationLeg of ORDINARY_RECEIPT_LEGS) {
      const second = receiptTransportStep({
        sourceState: receiptKatSource(
          firstPrefix,
          69,
          operationLeg,
        ),
        readResult: "POSITIVE_DATA",
        positiveByteKind: "LF",
        positiveTrailingSha256: receiptKatHash(70),
        lineParseResult: secondResult,
      });
      requireCondition(
        second.latch === null &&
          second.nextSourceState.observedPrefix === undefined &&
          receiptPrefixIndex(second.nextSourceState.roles) ===
            RECEIPT_PREFIX_NAMES.indexOf(wantedPrefix),
        "host.self_test",
        "second valid receipt continuation differs",
      );
      const eof = receiptTransportStep({
        sourceState: second.nextSourceState,
        readResult: "EOF",
      });
      requireCondition(
        eof.latch.classifiedRoute === "ORDINARY_SUCCESS" &&
          eof.latch.selectedLeg === operationLeg &&
          eof.latch.proofRoute ===
            RECEIPT_SUCCESS_PROOF_ROUTE[operationLeg] &&
          eof.latch.outcome === "ORDINARY_SUCCESS",
        "host.self_test",
        "ordinary receipt EOF dispatch differs",
      );
      ordinarySuccessCases += 1;
    }
  }
  requireCondition(
    ordinarySuccessCases === 6,
    "host.self_test",
    "ordinary receipt success case count differs",
  );

  let intentionalCases = 0;
  for (const [lineResult, operationLeg, role] of [
    ["VALID_P", "ONE_RECEIPT_P_FIRST", "P"],
    ["VALID_C", "ONE_RECEIPT_C_FIRST", "C"],
  ]) {
    const intentional = receiptTransportStep({
      sourceState: receiptKatSource(
        "NONE",
        69,
        operationLeg,
      ),
      readResult: "POSITIVE_DATA",
      positiveByteKind: "LF",
      positiveTrailingSha256: receiptKatHash(70),
      lineParseResult: lineResult,
      mode: "ONE_RECEIPT",
    });
    requireReceiptKatTerminal(
      intentional,
      "PROTOCOL",
      "PROTOCOL_INTENTIONAL_ONE_RECORD_STOP",
      "intentional one-receipt",
    );
    requireCondition(
      intentional.latch.observedPrefix === role &&
        intentional.latch.selectedLeg === operationLeg &&
        intentional.latch.trailingLength === 70 &&
        intentional.latch.outcome ===
          "ONE_RECEIPT_ELIGIBLE_AFTER_CLEANUP",
      "host.self_test",
      "intentional one-receipt payload differs",
    );
    intentionalCases += 1;
  }
  requireCondition(
    intentionalCases === 2,
    "host.self_test",
    "intentional receipt order count differs",
  );
  rejects(
    () =>
      receiptTransportStep({
        sourceState: receiptKatSource(
          "NONE",
          69,
          "ONE_RECEIPT_C_FIRST",
        ),
        readResult: "POSITIVE_DATA",
        positiveByteKind: "LF",
        positiveTrailingSha256: receiptKatHash(70),
        lineParseResult: "VALID_P",
        mode: "ONE_RECEIPT",
      }),
    "one-receipt order mismatch",
  );

  let partialEofCases = 0;
  for (const prefix of RECEIPT_PREFIX_NAMES) {
    const count = RECEIPT_PREFIX_COUNTS[prefix];
    for (const trailingLength of [1, 191]) {
      const step = receiptTransportStep({
        sourceState: receiptKatSource(prefix, trailingLength),
        readResult: "EOF",
      });
      requireCondition(
        step.latch.classifiedRoute ===
          RECEIPT_PARTIAL_EOF_ROUTE[count] &&
          step.latch.selectedLeg === "NONE" &&
          step.latch.outcome === "TYPED_STOP" &&
          step.latch.proofRoute ===
            ordinaryRedProofRoute(
              RECEIPT_PARTIAL_EOF_ROUTE[count],
              count,
            ),
        "host.self_test",
        "partial EOF route differs",
      );
      partialEofCases += 1;
    }
  }
  let plainEofCases = 0;
  for (const prefix of RECEIPT_PREFIX_NAMES) {
    const count = RECEIPT_PREFIX_COUNTS[prefix];
    const step = receiptTransportStep({
      sourceState: receiptKatSource(
        prefix,
        0,
        count === 2 ? "SUPPORT" : "DENIAL",
      ),
      readResult: "EOF",
    });
    requireCondition(
      step.latch.classifiedRoute ===
        (count === 0
          ? "ZERO_EOF"
          : count === 1
            ? "ORDINARY_ONE_RECORD_EOF"
            : "ORDINARY_SUCCESS"),
      "host.self_test",
      "plain EOF route differs",
    );
    plainEofCases += 1;
  }
  let readErrorCases = 0;
  let deadlineCases = 0;
  for (const prefix of RECEIPT_PREFIX_NAMES) {
    const count = RECEIPT_PREFIX_COUNTS[prefix];
    for (const trailingLength of [0, 1, 191]) {
      const readError = receiptTransportStep({
        sourceState: receiptKatSource(prefix, trailingLength),
        readResult: "NON_EAGAIN_READ_ERROR",
      });
      requireCondition(
        readError.latch.authorityPrefix === "NONE" &&
          readError.latch.classifiedRoute ===
            "NON_EAGAIN_READ_ERROR_NO_SEMANTIC_AUTHORITY" &&
          readError.latch.proofRoute ===
            "RECEIPT_NO_ANCHOR_DIRECT_ONLY",
        "host.self_test",
        "read-error authority differs",
      );
      readErrorCases += 1;
      const deadline = receiptTransportStep({
        sourceState: receiptKatSource(prefix, trailingLength),
        deadlineNs: 100n,
        preNowNs: 101n,
        postNowNs: 101n,
        readResult: "EOF",
      });
      requireCondition(
        deadline.latch.classifiedRoute ===
          RECEIPT_DEADLINE_ROUTE[count] &&
          deadline.latch.proofRoute === ordinaryRedProofRoute(
            RECEIPT_DEADLINE_ROUTE[count],
            count,
          ),
        "host.self_test",
        "deadline receipt route differs",
      );
      deadlineCases += 1;
    }
  }
  requireCondition(
    partialEofCases === 10 &&
      plainEofCases === 5 &&
      readErrorCases === 15 &&
      deadlineCases === 15,
    "host.self_test",
    "receipt EOF/read/deadline case counts differ",
  );

  const parserCapLatches = [];
  for (const prefix of RECEIPT_PREFIX_NAMES) {
    const step = receiptTransportStep({
      sourceState: receiptKatSource(prefix, 191),
      readResult: "POSITIVE_DATA",
      positiveByteKind: "NON_LF",
      positiveTrailingSha256: receiptKatHash(192),
      lineParseResult: "NONE",
    });
    requireReceiptKatTerminal(
      step,
      "CAPACITY",
      "CAPACITY_RECORD_193",
      "record cap",
    );
    requireCondition(
      step.latch.trailingLength === 192 &&
        step.reads === 1 &&
        step.committedBytes === 1,
      "host.self_test",
      "record cap accounting differs",
    );
    parserCapLatches.push(step.latch);
  }
  for (const prefix of ["P_C", "C_P"]) {
    for (const lineParseResult of [
      "VALID_P",
      "MALFORMED",
      prefix === "P_C" ? "VALID_P" : "VALID_C",
    ]) {
      const step = receiptTransportStep({
        sourceState: receiptKatSource(prefix, 69),
        readResult: "POSITIVE_DATA",
        positiveByteKind: "LF",
        positiveTrailingSha256: receiptKatHash(70),
        lineParseResult,
      });
      requireReceiptKatTerminal(
        step,
        "PROTOCOL",
        "PROTOCOL_THIRD_RECORD",
        "third record",
      );
      parserCapLatches.push(step.latch);
    }
  }
  let thirdWithoutParserCases = 0;
  for (const prefix of ["P_C", "C_P"]) {
    const step = receiptTransportStep({
      sourceState: receiptKatSource(prefix, 69),
      readResult: "POSITIVE_DATA",
      positiveByteKind: "LF",
      positiveTrailingSha256: receiptKatHash(70),
      lineParseResult: "NONE",
    });
    requireReceiptKatTerminal(
      step,
      "PROTOCOL",
      "PROTOCOL_THIRD_RECORD",
      "third record before parser result",
    );
    thirdWithoutParserCases += 1;
  }
  for (const [prefix, lineParseResult] of [
    ["NONE", "MALFORMED"],
    ["P", "MALFORMED"],
    ["C", "MALFORMED"],
    ["P", "VALID_P"],
    ["C", "VALID_C"],
  ]) {
    const step = receiptTransportStep({
      sourceState: receiptKatSource(prefix, 69),
      readResult: "POSITIVE_DATA",
      positiveByteKind: "LF",
      positiveTrailingSha256: receiptKatHash(70),
      lineParseResult,
    });
    requireReceiptKatTerminal(
      step,
      "GRAMMAR",
      "GRAMMAR_MALFORMED_OR_DUPLICATE",
      "malformed or duplicate receipt",
    );
    parserCapLatches.push(step.latch);
  }
  requireCondition(
    parserCapLatches.length === 16,
    "host.self_test",
    "parser/cap terminal case count differs",
  );
  const derivedAggregateMaximum = 70 * 2 + 192;
  const derivedFirstUnreachable = derivedAggregateMaximum + 1;
  requireCondition(
    derivedAggregateMaximum === 332 &&
      derivedFirstUnreachable === 333 &&
      derivedFirstUnreachable < 384 &&
      385 > derivedFirstUnreachable,
    "host.self_test",
    "receipt aggregate 332/333/385 derivation differs",
  );
  let exact192PartialEofAbsenceCases = 0;
  for (const prefix of RECEIPT_PREFIX_NAMES) {
    rejects(
      () =>
        receiptTransportStep({
          sourceState: receiptKatSource(prefix, 192),
          readResult: "EOF",
        }),
      "exact-192 partial EOF",
    );
    exact192PartialEofAbsenceCases += 1;
  }
  let laterCompanionCases = 0;
  for (const latch of parserCapLatches) {
    for (const later of [
      { readResult: "EOF" },
      { readResult: "NON_EAGAIN_READ_ERROR" },
      {
        deadlineNs: 100n,
        preNowNs: 101n,
        postNowNs: 101n,
        readResult: "EOF",
      },
    ]) {
      const same = receiptTransportStep({ latched: latch, ...later });
      requireCondition(
        same.latch === latch &&
          same.reads === 0 &&
          same.reservationAttempts === 0,
        "host.self_test",
        "latched receipt accepted a later event",
      );
      laterCompanionCases += 1;
    }
  }
  requireCondition(
    laterCompanionCases === 48,
    "host.self_test",
    "parser/cap companion case count differs",
  );

  let adjacentCases = 0;
  let exact192LfCases = 0;
  for (const prefix of ["NONE", "P", "C"]) {
    const step = receiptTransportStep({
      sourceState: receiptKatSource(prefix, 191),
      readResult: "POSITIVE_DATA",
      positiveByteKind: "LF",
      positiveTrailingSha256: receiptKatHash(192),
      lineParseResult: "MALFORMED",
    });
    requireCondition(
      step.latch.terminal ===
        "GRAMMAR_MALFORMED_OR_DUPLICATE" &&
        step.latch.terminal !== "CAPACITY_RECORD_193",
      "host.self_test",
      "length-192 LF did not proceed to grammar",
    );
    exact192LfCases += 1;
  }
  for (const prefix of ["P_C", "C_P"]) {
    for (const trailingLength of [0, 1, 191]) {
      const step = receiptTransportStep({
        sourceState: receiptKatSource(prefix, trailingLength),
        readResult: "EOF",
      });
      requireCondition(
        step.latch.classifiedRoute ===
          (trailingLength === 0
            ? "ORDINARY_SUCCESS"
            : "TWO_VALID_PLUS_PARTIAL_EOF"),
        "host.self_test",
        "two-role adjacent EOF differs",
      );
      adjacentCases += 1;
    }
    for (const [trailingLength, byteKind] of [
      [191, "NON_LF"],
      [191, "LF"],
    ]) {
      const step = receiptTransportStep({
        sourceState: receiptKatSource(prefix, trailingLength),
        readResult: "POSITIVE_DATA",
        positiveByteKind: byteKind,
        positiveTrailingSha256: receiptKatHash(192),
        lineParseResult:
          byteKind === "LF" ? "MALFORMED" : "NONE",
      });
      requireCondition(
        step.latch.classifiedRoute ===
          (byteKind === "LF"
            ? "MALFORMED_DUPLICATE_OR_THIRD"
            : "RECORD_OVER_CAP"),
        "host.self_test",
        "two-role adjacent cap/third differs",
      );
      adjacentCases += 1;
    }
  }
  requireCondition(
    adjacentCases === 10,
    "host.self_test",
    "two-role adjacent case count differs",
  );

  const redSources = [
    ["EOF_TERMINAL", "NONE", 0, "ZERO_EOF"],
    [
      "READ_NON_EAGAIN_ERROR",
      "P_C",
      191,
      "NON_EAGAIN_READ_ERROR_NO_SEMANTIC_AUTHORITY",
    ],
    [
      "DEADLINE_NO_EOF",
      "NONE",
      0,
      "DEADLINE_ZERO_COMPLETE_VALID_ROLE",
    ],
    [
      "DEADLINE_NO_EOF",
      "P",
      1,
      "DEADLINE_ONE_COMPLETE_VALID_ROLE",
    ],
    [
      "DEADLINE_NO_EOF",
      "P_C",
      1,
      "DEADLINE_TWO_COMPLETE_VALID_ROLES",
    ],
    ["EOF_TERMINAL", "NONE", 1, "PARTIAL_NO_LF_EOF"],
    [
      "EOF_TERMINAL",
      "P",
      1,
      "ONE_VALID_PLUS_PARTIAL_EOF",
    ],
    [
      "EOF_TERMINAL",
      "P_C",
      1,
      "TWO_VALID_PLUS_PARTIAL_EOF",
    ],
    [
      "GRAMMAR_MALFORMED_OR_DUPLICATE",
      "P",
      70,
      "MALFORMED_DUPLICATE_OR_THIRD",
    ],
    ["CAPACITY_RECORD_193", "P", 192, "RECORD_OVER_CAP"],
    [
      "EOF_TERMINAL",
      "P",
      0,
      "ORDINARY_ONE_RECORD_EOF",
    ],
  ];
  let redOperationLegCases = 0;
  for (const [terminal, prefix, length, route] of redSources) {
    for (const operationLeg of ORDINARY_RECEIPT_LEGS) {
      const latch = latchReceiptTerminal(
        null,
        terminal,
        receiptKatSource(prefix, length, operationLeg),
      );
      requireCondition(
        latch.classifiedRoute === route &&
          latch.selectedLeg === "NONE" &&
          latch.outcome === "TYPED_STOP",
        "host.self_test",
        "red receipt operation-leg sanitization differs",
      );
      redOperationLegCases += 1;
    }
  }
  requireCondition(
    redOperationLegCases === 33,
    "host.self_test",
    "red receipt operation-leg case count differs",
  );
  requireCondition(
    canonicalJson(
      [...new Set(redSources.map((entry) => entry[3]))].sort(),
    ) ===
      canonicalJson(
        Object.keys(RECEIPT_RED_PROOF_ROUTE).sort(),
      ),
    "host.self_test",
    "ordinary red receipt route-map coverage differs",
  );

  const ownerLatch = latchReceiptTerminal(
    null,
    "EOF_TERMINAL",
    receiptKatSource("NONE", 0),
  );
  const intentionalOwnerLatch = latchReceiptTerminal(
    null,
    "PROTOCOL_INTENTIONAL_ONE_RECORD_STOP",
    receiptKatSource(
      "P",
      70,
      "ONE_RECEIPT_P_FIRST",
    ),
  );
  const readErrorOwnerLatch = receiptTransportStep({
    readResult: "NON_EAGAIN_READ_ERROR",
  }).latch;
  const standaloneLatches = [
    ["intentional", intentionalOwnerLatch],
    ["read-error", readErrorOwnerLatch],
    ["EOF", ownerLatch],
    ["pre-deadline", preExpired.latch],
    ["post-deadline", postExpired.latch],
  ];
  let hostileStandaloneCases = 0;
  for (const [label, latch] of standaloneLatches) {
    let hostileLaterGetterCalls = 0;
    const hostileLater = { latched: latch };
    Object.defineProperty(hostileLater, "readResult", {
      enumerable: true,
      get() {
        hostileLaterGetterCalls += 1;
        throw new Error(`later receipt getter ${label}`);
      },
    });
    const existing = receiptTransportStep(hostileLater);
    requireCondition(
      existing.latch === latch &&
        existing.reads === 0 &&
        existing.reservationAttempts === 0 &&
        hostileLaterGetterCalls === 0,
      "host.self_test",
      `existing ${label} latch did not win before later fields`,
    );
    hostileStandaloneCases += 1;
  }
  rejects(
    () =>
      latchReceiptTerminal(
        ownerLatch,
        "READ_NON_EAGAIN_ERROR",
        receiptKatSource("NONE"),
      ),
    "receipt second-set",
  );
  rejects(
    () =>
      latchReceiptTerminal(
        null,
        "UNKNOWN_TERMINAL",
        receiptKatSource("NONE"),
      ),
    "receipt unknown terminal",
  );
  let incompatiblePayloadCases = 0;
  for (const [field, value] of [
    ["family", "READ"],
    ["observedPrefix", "P"],
    ["authorityPrefix", "P"],
    ["trailingLength", 1],
    ["trailingSha256", receiptKatHash(1)],
    ["selectedLeg", "SUPPORT"],
    ["classifiedRoute", "ORDINARY_SUCCESS"],
    ["proofRoute", "support"],
    ["outcome", "ORDINARY_SUCCESS"],
  ]) {
    rejects(
      () =>
        validateReceiptLatchPayload({
          ...ownerLatch,
          [field]: value,
        }),
      `receipt incompatible payload ${field}`,
    );
    incompatiblePayloadCases += 1;
  }
  const mutableSource = {
    roles: [],
    trailingLength: 0,
    trailingSha256: NO_TRAILING_SHA256,
    operationLeg: "SUPPORT",
  };
  const copiedLatch = latchReceiptTerminal(
    null,
    "EOF_TERMINAL",
    mutableSource,
  );
  mutableSource.roles.push("P");
  mutableSource.trailingLength = 191;
  mutableSource.trailingSha256 = receiptKatHash(191);
  mutableSource.operationLeg = "DENIAL";
  requireCondition(
    Object.isFrozen(copiedLatch) &&
      copiedLatch.observedPrefix === "NONE" &&
      copiedLatch.trailingLength === 0 &&
      copiedLatch.selectedLeg === "NONE" &&
      copiedLatch.classifiedRoute === "ZERO_EOF",
    "host.self_test",
    "receipt latch changed after caller-state mutation",
  );
  rejects(
    () =>
      receiptTransportStep({
        latched: Object.freeze({ ...ownerLatch }),
        readResult: "EOF",
      }),
    "forged frozen receipt latch",
  );
  rejects(
    () =>
      receiptTransportStep({
        ...receiptKatSource("NONE"),
        readResult: "EOF",
      }),
    "receipt transport alien source fields",
  );
  let transportGetterCalls = 0;
  const transportGetter = {};
  Object.defineProperty(transportGetter, "readResult", {
    enumerable: true,
    get() {
      transportGetterCalls += 1;
      return "EOF";
    },
  });
  rejects(
    () => receiptTransportStep(transportGetter),
    "receipt transport getter",
  );
  requireCondition(
    transportGetterCalls === 0,
    "host.self_test",
    "receipt transport getter was invoked",
  );
  const transportProxy = new Proxy({}, {
    get() {
      throw new Error("receipt transport proxy get");
    },
  });
  rejects(
    () => receiptTransportStep(transportProxy),
    "receipt transport proxy",
  );

  requireCondition(
    parseRubyReceiptCandidate(
      Buffer.from("P|S|123|122|123|123|16777234|9007199254740993\n"),
    ).result === "VALID_P" &&
      parseRubyReceiptCandidate(
        Buffer.from("C|D|124|123|123|123|bind|1\n"),
      ).result === "VALID_C" &&
      parseRubyReceiptCandidate(
        Buffer.from("P|D|123|122|123|123|bind|2\n"),
      ).result === "MALFORMED" &&
      parseRubyReceiptCandidate(
        Buffer.concat([Buffer.alloc(70, 0x61), Buffer.from("\n")]),
      ).result === "MALFORMED",
    "host.self_test",
    "closed Ruby receipt line parser differs",
  );
  const standaloneTerminalCount = new Set(
    standaloneLatches.map(([, latch]) => latch.terminal),
  ).size;
  requireCondition(
    deadlineMatrixCases === 24 &&
      thirdWithoutParserCases === 2 &&
      exact192PartialEofAbsenceCases === 5 &&
      exact192LfCases === 3 &&
      hostileStandaloneCases === 5 &&
      standaloneTerminalCount === 4 &&
      incompatiblePayloadCases === 9,
    "host.self_test",
    "receipt recurrence matrix aggregate count differs",
  );

  return Object.freeze({
    deadlineMatrixCases,
    partialEofCases,
    plainEofCases,
    ordinarySuccessCases,
    intentionalCases,
    readErrorCases,
    deadlineCases,
    parserCapCases: parserCapLatches.length,
    thirdWithoutParserCases,
    exact192PartialEofAbsenceCases,
    exact192LfCases,
    laterCompanionCases,
    adjacentCases,
    redOperationLegCases,
    hostileStandaloneCases,
    standaloneTerminals: standaloneTerminalCount,
    incompatiblePayloadCases,
    derivedAggregateMaximum,
    derivedFirstUnreachable,
  });
}

const SOURCE_RECURRENCE_FAMILIES = Object.freeze([
  "RECEIPT_POST_RETURN_BEFORE_INSPECTION",
  "DECLARED_RECEIPT_CUSTODY",
  "BRANCH_SPECIFIC_RUBY_CLEANUP",
  "CHARGE_BEFORE_MUTATION",
  "POST_FSTAT_DEADLINE",
  "WORKER_BASIS_OUTER_DEADLINE",
]);

function sourceRecurrenceDefects() {
  const defects = [];
  const receiptSource = rubyReceiptReader.toString();
  const receiptOwnerSource = readReceiptByteOwned.toString();
  const receiptReadIndex =
    receiptOwnerSource.indexOf("count = readOne(");
  const receiptPostIndex =
    receiptOwnerSource.indexOf("postNowNs = sampleNow()");
  const receiptGuardIndex = receiptOwnerSource.indexOf(
    "if (postNowNs < preNowNs || postNowNs > deadlineNs)",
  );
  const receiptCommitIndex = receiptOwnerSource.indexOf(
    "counters.committedBytes += 1",
  );
  const receiptCopyIndex = receiptOwnerSource.indexOf(
    "positiveByte = Buffer.from(scratch)",
  );
  if (
    !receiptSource.includes("readReceiptByteOwned(") ||
    receiptSource.indexOf("readReceiptByteOwned(") >
      receiptSource.indexOf("receiptTransportStep(") ||
    receiptOwnerSource.indexOf(
      "counters.reservationAttempts += 1",
    ) >
      receiptOwnerSource.indexOf("count = readOne(") ||
    receiptReadIndex < 0 ||
    receiptPostIndex <= receiptReadIndex ||
    receiptGuardIndex <= receiptPostIndex ||
    receiptCommitIndex <= receiptGuardIndex ||
    receiptCopyIndex <= receiptCommitIndex ||
    (receiptOwnerSource.match(/\bscratch\b/gu) ?? []).length !== 3 ||
    receiptOwnerSource
      .slice(receiptPostIndex, receiptGuardIndex)
      .includes("scratch") ||
    !receiptOwnerSource.includes("allocateOne = () =>")
  ) {
    defects.push("RECEIPT_POST_RETURN_BEFORE_INSPECTION");
  }

  const cleanupSource =
    RubyBranchCleanupOwner.prototype.cleanup.toString();
  if (
    !cleanupSource.includes(
      "this.proofOwner.captureReceiptFaultCustody(",
    ) ||
    !cleanupSource.includes("canonicalJson(custody)") ||
    !RubyProofOwner.prototype.captureReceiptFaultCustody
      .toString()
      .includes("receiptFaultCustodyReceipt(")
  ) {
    defects.push("DECLARED_RECEIPT_CUSTODY");
  }

  const rubyLegSource = runRubyLeg.toString();
  const cleanupOwnerIndex = rubyLegSource.indexOf(
    "new RubyBranchCleanupOwner(",
  );
  const topologyIndex = rubyLegSource.indexOf(
    "for (const record of receiptRecords)",
  );
  if (
    cleanupOwnerIndex < 0 ||
    topologyIndex <= cleanupOwnerIndex ||
    !rubyLegSource.includes("cleanupOwner.run(") ||
    !cleanupSource.includes("rubyCleanupSuffix(") ||
    !cleanupSource.includes("for (const ordinal of remaining.filter(") ||
    !RubyBranchCleanupOwner.prototype.retireFifo
      .toString()
      .includes("this.receiptClosed = true")
  ) {
    defects.push("BRANCH_SPECIFIC_RUBY_CLEANUP");
  }

  const directorySource = createDirectory.toString();
  const fifoSource = FifoManager.prototype.create.toString();
  const nodeSource = runNodeLeg.toString();
  const invocationSource = createInvocation.toString();
  const evidenceSource = createEvidenceFile.toString();
  const canarySource = createCanary.toString();
  const rubySource = runRubyLeg.toString();
  const proofSource = RubyProofOwner.prototype.capture.toString();
  if (
    typeof CapacityLedger !== "function" ||
    invocationSource.indexOf("reserveDirectory(") < 0 ||
    invocationSource.indexOf("reserveDirectory(") >
      invocationSource.indexOf("mkdtempSync(") ||
    directorySource.indexOf("reserveDirectory(") < 0 ||
    directorySource.indexOf("reserveDirectory(") >
      directorySource.indexOf("mkdirSync(") ||
    evidenceSource.indexOf("reserveRegularFile(") < 0 ||
    evidenceSource.indexOf("reserveRegularFile(") >
      evidenceSource.indexOf("openSync(") ||
    canarySource.indexOf("reserveRegularFile(") < 0 ||
    canarySource.indexOf("reserveRegularFile(") >
      canarySource.indexOf("openSync(") ||
    fifoSource.indexOf("reserveFifoBatch(") < 0 ||
    fifoSource.indexOf("reserveFifoBatch(") >
      fifoSource.indexOf("createDirectory(") ||
    nodeSource.indexOf("reserveNodeLeg(") < 0 ||
    nodeSource.indexOf("reserveNodeLeg(") >
      nodeSource.indexOf("createDirectory(") ||
    rubySource.indexOf("reserveRubyLeg(") < 0 ||
    rubySource.indexOf("reserveRubyLeg(") >
      rubySource.indexOf("createRubyRoots(") ||
    proofSource.indexOf("reserveProof(") < 0 ||
    proofSource.indexOf("reserveProof(") >
      proofSource.indexOf("checkCanary(") ||
    !runProductionCapacityRefusalSelfTests
      .toString()
      .includes("runNodeLeg(") ||
    !runProductionCapacityRefusalSelfTests
      .toString()
      .includes("runRubyLeg(") ||
    !runProductionCapacityRefusalSelfTests
      .toString()
      .includes("proofOwner.capture(")
  ) {
    defects.push("CHARGE_BEFORE_MUTATION");
  }

  const staticSource = streamRegular.toString();
  const ownerSource = streamOwner.toString();
  const boundedSource = readBoundedRegular.toString();
  const fstatSiblingSources = [
    openEvidenceAppend,
    createEvidenceFile,
    openDevNull,
    fifoReceipt,
    openFifoEndpoints,
    verifyFifoEofAndClose,
    createCanary,
    checkCanary,
    retireCanary,
  ].map((owner) => owner.toString());
  if (
    !staticSource.includes("fstatUnderDeadline(") ||
    !ownerSource.includes("fstatUnderDeadline(") ||
    !boundedSource.includes("fstatUnderDeadline(") ||
    fstatSiblingSources.some(
      (source) => !source.includes("fstatUnderDeadline("),
    ) ||
    `${staticSource}${ownerSource}${boundedSource}${fstatSiblingSources.join("")}`
      .includes("statFact(fstatBig(")
  ) {
    defects.push("POST_FSTAT_DEADLINE");
  }

  const workerArgvSource = verifyWorkerArguments.toString();
  const workerSource = runWorker.toString();
  if (
    !workerArgvSource.includes("args.length === 11") ||
    !workerArgvSource.includes("/^chain:(") ||
    !workerArgvSource.includes("/^basis:(") ||
    !workerArgvSource.includes("outerDeadline") ||
    !workerArgvSource.includes(
      "parsed.workerDeadline <= parsed.outerDeadline",
    ) ||
    workerSource.includes('new AbsoluteDeadline(\n        "worker-failure-report"')
  ) {
    defects.push("WORKER_BASIS_OUTER_DEADLINE");
  }

  return Object.freeze(defects);
}

function runReceiptByteOwnerSelfTests() {
  const counters = () => ({
    reads: 0,
    reservationAttempts: 0,
    reservationReleases: 0,
    committedBytes: 0,
  });
  const sampled = (values) => () => values.shift();

  const timelyCounters = counters();
  const timely = readReceiptByteOwned({
    fd: 7,
    deadlineNs: 10n,
    counters: timelyCounters,
    sampleNow: sampled([9n, 10n]),
    readOne(_fd, buffer) {
      buffer[0] = 0x41;
      return 1;
    },
  });
  requireCondition(
    timely.readResult === "POSITIVE_DATA" &&
      timely.positiveByte.equals(Buffer.from("A")) &&
      canonicalJson(timelyCounters) ===
        canonicalJson({
          reads: 1,
          reservationAttempts: 1,
          reservationReleases: 0,
          committedBytes: 1,
        }),
    "host.self_test",
    "receipt byte owner equality/commit KAT differs",
  );

  let lateCountInspections = 0;
  let lateScratchInspections = 0;
  const lateCountCounters = counters();
  const lateCount = readReceiptByteOwned({
    fd: 7,
    deadlineNs: 10n,
    counters: lateCountCounters,
    sampleNow: sampled([9n, 11n]),
    readOne() {
      return new Proxy(
        {},
        {
          get() {
            lateCountInspections += 1;
            throw new Error("late count inspected");
          },
        },
      );
    },
    allocateOne() {
      return new Proxy(Buffer.alloc(1), {
        get(target, property, receiver) {
          lateScratchInspections += 1;
          return Reflect.get(target, property, receiver);
        },
      });
    },
  });
  requireCondition(
    lateCount.readResult === undefined &&
      lateCount.positiveByte === undefined &&
      lateCountInspections === 0 &&
      lateScratchInspections === 0 &&
      lateCountCounters.reservationReleases === 1 &&
      lateCountCounters.committedBytes === 0,
    "host.self_test",
    "late receipt count was inspected or retained",
  );

  let lateErrorInspections = 0;
  const lateErrorCounters = counters();
  const hostileError = new Proxy(
    {},
    {
      get() {
        lateErrorInspections += 1;
        throw new Error("late error inspected");
      },
    },
  );
  const lateError = readReceiptByteOwned({
    fd: 7,
    deadlineNs: 10n,
    counters: lateErrorCounters,
    sampleNow: sampled([9n, 11n]),
    readOne() {
      throw hostileError;
    },
  });
  requireCondition(
    lateError.readResult === undefined &&
      !lateError.nonEagainReadError &&
      lateErrorInspections === 0 &&
      lateErrorCounters.reservationReleases === 1,
    "host.self_test",
    "late receipt error was inspected",
  );

  let preLateReads = 0;
  const preLateCounters = counters();
  const preLate = readReceiptByteOwned({
    fd: 7,
    deadlineNs: 10n,
    counters: preLateCounters,
    sampleNow: sampled([11n]),
    readOne() {
      preLateReads += 1;
      return 0;
    },
  });
  requireCondition(
    preLate.readResult === undefined &&
      preLateReads === 0 &&
      preLateCounters.reservationAttempts === 0,
    "host.self_test",
    "pre-expired receipt owner performed a read",
  );

  const regressionCounters = counters();
  const regression = readReceiptByteOwned({
    fd: 7,
    deadlineNs: 10n,
    counters: regressionCounters,
    sampleNow: sampled([9n, 8n]),
    readOne(_fd, buffer) {
      buffer[0] = 0x42;
      return 1;
    },
  });
  requireCondition(
    regression.readResult === undefined &&
      regression.positiveByte === undefined &&
      regressionCounters.reservationReleases === 1 &&
      regressionCounters.committedBytes === 0,
    "host.self_test",
    "regressed receipt clock retained a byte",
  );

  const eagainCounters = counters();
  const eagain = readReceiptByteOwned({
    fd: 7,
    deadlineNs: 10n,
    counters: eagainCounters,
    sampleNow: sampled([9n, 10n]),
    readOne() {
      const error = new Error("again");
      error.code = "EAGAIN";
      throw error;
    },
  });
  requireCondition(
    eagain.readResult === "EAGAIN" &&
      eagainCounters.reservationReleases === 1 &&
      eagainCounters.committedBytes === 0,
    "host.self_test",
    "timely EAGAIN receipt KAT differs",
  );

  const eofCounters = counters();
  const eof = readReceiptByteOwned({
    fd: 7,
    deadlineNs: 10n,
    counters: eofCounters,
    sampleNow: sampled([9n, 10n]),
    readOne() {
      return 0;
    },
  });
  requireCondition(
    eof.readResult === "EOF" &&
      eof.positiveByte === undefined &&
      eofCounters.reservationReleases === 1 &&
      eofCounters.committedBytes === 0,
    "host.self_test",
    "timely EOF receipt KAT differs",
  );

  const errorCounters = counters();
  const timelyError = readReceiptByteOwned({
    fd: 7,
    deadlineNs: 10n,
    counters: errorCounters,
    sampleNow: sampled([9n, 10n]),
    readOne() {
      const error = new Error("read fault");
      error.code = "EIO";
      throw error;
    },
  });
  requireCondition(
    timelyError.readResult === "NON_EAGAIN_READ_ERROR" &&
      timelyError.nonEagainReadError &&
      errorCounters.reservationReleases === 1 &&
      errorCounters.committedBytes === 0,
    "host.self_test",
    "timely non-EAGAIN receipt KAT differs",
  );

  const impossibleCounters = counters();
  rejects(
    () =>
      readReceiptByteOwned({
        fd: 7,
        deadlineNs: 10n,
        counters: impossibleCounters,
        sampleNow: sampled([9n, 10n]),
        readOne() {
          return 2;
        },
      }),
    "receipt impossible count",
  );
  requireCondition(
    impossibleCounters.reservationAttempts === 1 &&
      impossibleCounters.reservationReleases === 1 &&
      impossibleCounters.committedBytes === 0,
    "host.self_test",
    "impossible receipt count capacity disposition differs",
  );
  return Object.freeze({ cases: 9 });
}

function runCapacityLedgerSelfTests() {
  const owners = Object.keys(CAPACITY_MAXIMA);
  let boundaryCases = 0;
  for (const owner of owners) {
    const maximum = CAPACITY_MAXIMA[owner];
    const atBoundary = new CapacityLedger({
      [owner]: maximum - 1,
    });
    let mutations = 0;
    const reservation = atBoundary.reserve(owner);
    mutations += 1;
    atBoundary.complete(reservation);
    requireCondition(
      mutations === 1 &&
        atBoundary.reserved[owner] === maximum &&
        atBoundary.completed[owner] === maximum,
      "host.self_test",
      `capacity ${owner} N boundary differs`,
    );
    rejects(
      () => atBoundary.complete(reservation),
      `capacity ${owner} duplicate completion`,
    );

    const atOverflow = new CapacityLedger({
      [owner]: maximum,
    });
    let forbiddenMutation = 0;
    rejects(() => {
      atOverflow.reserve(owner);
      forbiddenMutation += 1;
    }, `capacity ${owner} N+1`);
    requireCondition(
      forbiddenMutation === 0 &&
        atOverflow.reserved[owner] === maximum &&
        atOverflow.completed[owner] === maximum,
      "host.self_test",
      `capacity ${owner} N+1 mutated production state`,
    );
    boundaryCases += 2;
  }
  const zero = new CapacityLedger().snapshot();
  requireCondition(
    owners.length === 17 &&
      boundaryCases === 34 &&
      owners.every(
        (owner) =>
          zero.reserved[owner] === 0 &&
          zero.completed[owner] === 0,
      ),
    "host.self_test",
    "closed capacity owner inventory differs",
  );
  return Object.freeze({ owners: owners.length, boundaryCases });
}

async function runProductionCapacityRefusalSelfTests() {
  const root = `/private/tmp/marrow-vsq-a-capacity-${process.pid}`;
  const fifoRootPath = `${root}/fifo`;
  const deadline = {
    endsNs: MAX_U64,
    check() {},
    sub() {
      return this;
    },
  };
  const saturated = (owner) =>
    new CapacityLedger({
      [owner]: CAPACITY_MAXIMA[owner],
    });
  let cases = 0;
  const exactAbsence = () => {
    requireCondition(
      absentNoFollow(root) &&
        absentNoFollow(root),
      "host.self_test",
      "capacity refusal created its forbidden task root",
      { pathHash: sha256(Buffer.from(root)) },
    );
  };
  exactAbsence();

  await rejectsAsync(
    () =>
      Promise.resolve(
        createInvocation(
          "a".repeat(64),
          deadline,
          saturated("directories"),
        ),
      ),
    "invocation directory capacity N+1",
  );
  cases += 1;
  rejects(
    () =>
      createDirectory(
        root,
        "capacity-kat",
        saturated("directories"),
      ),
    "directory capacity N+1",
  );
  cases += 1;
  rejects(
    () =>
      createEvidenceFile(
        {
          evidenceRoot: root,
          capacity: saturated("regularFiles"),
        },
        deadline,
      ),
    "evidence capacity N+1",
  );
  cases += 1;
  rejects(
    () =>
      createCanary(
        {
          capacity: saturated("regularFiles"),
        },
        Object.freeze({ path: root }),
        deadline,
      ),
    "canary capacity N+1",
  );
  cases += 1;

  for (const owner of [
    "fifoBatches",
    "fifoInodes",
    "fifoPathBytes",
  ]) {
    const capacity = saturated(owner);
    const manager = new FifoManager(
      {
        capacity,
        invocation: root,
        fifoFacts: [],
      },
      Object.freeze({ path: fifoRootPath }),
      {},
      {},
    );
    await rejectsAsync(
      () =>
        manager.create(
          0,
          ["stdout.fifo", "stderr.fifo"],
          deadline,
        ),
      `FIFO ${owner} capacity N+1`,
    );
    requireCondition(
      manager.used.size === 0,
      "host.self_test",
      `FIFO ${owner} refusal consumed its mutation ordinal`,
    );
    cases += 1;
  }

  for (const [owner, support] of [
    ["nodeLegs", false],
    ["sockets", true],
  ]) {
    const capacity = saturated(owner);
    await rejectsAsync(
      () =>
        runNodeLeg(
          {
            deadline,
            capacity,
            preflight: root,
            invocation: root,
          },
          undefined,
          undefined,
          undefined,
          support,
        ),
      `Node ${owner} capacity N+1`,
    );
    cases += 1;
  }

  for (const [owner, support] of [
    ["rubyLegs", false],
    ["sockets", true],
  ]) {
    const capacity = saturated(owner);
    await rejectsAsync(
      () =>
        runRubyLeg(
          {
            deadline,
            capacity,
          },
          undefined,
          undefined,
          undefined,
          undefined,
          0,
          support ? "support" : "denial",
          Object.freeze({ path: root }),
        ),
      `Ruby ${owner} capacity N+1`,
    );
    cases += 1;
  }

  for (const owner of ["proofs", "psCaptures"]) {
    const capacity = saturated(owner);
    const proofOwner = new RubyProofOwner(
      { capacity },
      undefined,
      undefined,
      undefined,
      undefined,
    );
    await rejectsAsync(
      () =>
        proofOwner.capture(
          {
            branch: Object.freeze([1]),
            consumed: new Set(),
            lastOrdinal: 0,
            leg: "support",
          },
          1,
          () => true,
          "capacity-kat",
        ),
      `proof ${owner} capacity N+1`,
    );
    cases += 1;
  }

  exactAbsence();
  requireCondition(
    cases === 13,
    "host.self_test",
    "production capacity refusal case inventory differs",
    { cases },
  );
  return Object.freeze({
    cases,
    taskRootCreated: false,
  });
}

function fakeRegularStat(accesses) {
  return new Proxy(
    {
      dev: 1n,
      ino: 2n,
      mode: 0o100600n,
      uid: 501n,
      gid: 20n,
      size: 0n,
      nlink: 1n,
      isFile: () => true,
      isDirectory: () => false,
      isFIFO: () => false,
      isSocket: () => false,
      isCharacterDevice: () => false,
      isSymbolicLink: () => false,
    },
    {
      get(target, property, receiver) {
        accesses.push(String(property));
        return Reflect.get(target, property, receiver);
      },
    },
  );
}

function runFstatDeadlineSelfTests() {
  const lateAccesses = [];
  rejects(
    () =>
      fstatUnderDeadline(
        7,
        {
          check() {
            throw new HostAuthorityError(
              "host.deadline",
              "held fstat crossed deadline",
            );
          },
        },
        "held",
        () => fakeRegularStat(lateAccesses),
      ),
    "late fstat inspection",
  );
  requireCondition(
    lateAccesses.length === 0,
    "host.self_test",
    "late fstat result was observed before refusal",
    { lateAccesses },
  );

  const atEquality = [];
  const equalityDeadline = {
    endsNs: 10n,
    nowNs: 9n,
    check() {
      atEquality.push("deadline");
      requireCondition(
        this.nowNs <= this.endsNs,
        "host.deadline",
        "fake fstat deadline expired",
      );
    },
  };
  const fact = fstatUnderDeadline(
    7,
    equalityDeadline,
    "equality",
    () => {
      atEquality.push("return");
      equalityDeadline.nowNs = 10n;
      return fakeRegularStat(atEquality);
    },
  );
  requireCondition(
    atEquality[0] === "return" &&
      atEquality[1] === "deadline" &&
      atEquality.length > 2 &&
      fact.type === "file" &&
      fact.dev === "1" &&
      fact.ino === "2",
    "host.self_test",
    "post-fstat deadline/inspection order differs",
    { atEquality },
  );

  const plusOneAccesses = [];
  const plusOneDeadline = {
    endsNs: 10n,
    nowNs: 9n,
    check() {
      requireCondition(
        this.nowNs <= this.endsNs,
        "host.deadline",
        "fake fstat deadline expired",
      );
    },
  };
  rejects(
    () =>
      fstatUnderDeadline(
        7,
        plusOneDeadline,
        "plus-one",
        () => {
          plusOneDeadline.nowNs = 11n;
          return fakeRegularStat(plusOneAccesses);
        },
      ),
    "plus-one fstat deadline",
  );
  requireCondition(
    plusOneAccesses.length === 0,
    "host.self_test",
    "plus-one fstat result was inspected",
    { plusOneAccesses },
  );
  return Object.freeze({
    lateInspectionCount: 0,
    equality: true,
    plusOneRefused: true,
  });
}

function workerArgvFixture() {
  return [
    "a".repeat(64),
    "/private/tmp/marrow-vsq-a-aaaaaaaa.ABCDEF",
    "1",
    "2",
    "3",
    "4",
    `chain:${"b".repeat(64)}`,
    "500000000001",
    "500000000000",
    "420000000000",
    `basis:${"c".repeat(64)}`,
  ];
}

function runWorkerContractSelfTests() {
  const fixture = workerArgvFixture();
  const parsed = verifyWorkerArguments(fixture);
  const owner = new WorkerDeadlineOwner(parsed);
  owner.confirm({
    basisToken: parsed.basisToken,
    outerDeadlineNs: parsed.outerDeadline.toString(),
    workerDeadlineNs: parsed.workerDeadline.toString(),
    workerRemainingNs: parsed.workerRemaining.toString(),
  });
  requireCondition(
    owner.requireConfirmed() === owner.deadline,
    "host.self_test",
    "worker deadline owner did not preserve its single instance",
  );

  const equalFixture = [...fixture];
  equalFixture[7] = fixture[8];
  const equal = verifyWorkerArguments(equalFixture);
  requireCondition(
    equal.workerDeadline === equal.outerDeadline,
    "host.self_test",
    "worker/outer deadline equality was refused",
  );

  const variants = [];
  variants.push(fixture.slice(0, -1));
  variants.push([...fixture, "extra"]);
  const extended = [...fixture];
  extended[8] = "500000000002";
  variants.push(extended);
  for (const [index, value] of [
    [7, "-1"],
    [7, "+1"],
    [7, "0500000000001"],
    [7, "18446744073709551616"],
    [8, "0"],
    [9, "420000000001"],
    [10, `chain:${"b".repeat(64)}`],
  ]) {
    const variant = [...fixture];
    variant[index] = value;
    variants.push(variant);
  }
  const typeCompatibleReorder = [...fixture];
  [
    typeCompatibleReorder[6],
    typeCompatibleReorder[10],
  ] = [
    typeCompatibleReorder[10],
    typeCompatibleReorder[6],
  ];
  variants.push(typeCompatibleReorder);
  for (const [index, variant] of variants.entries()) {
    rejects(
      () => verifyWorkerArguments(variant),
      `worker argv negative ${index}`,
    );
  }

  const mismatchOwner = new WorkerDeadlineOwner(parsed);
  rejects(
    () =>
      mismatchOwner.confirm({
        basisToken: "d".repeat(64),
        outerDeadlineNs: parsed.outerDeadline.toString(),
        workerDeadlineNs: parsed.workerDeadline.toString(),
        workerRemainingNs: parsed.workerRemaining.toString(),
      }),
    "worker basis mismatch",
  );
  return Object.freeze({
    exactFields: fixture.length,
    negativeCases: variants.length + 1,
  });
}

function runCustodySelfTests() {
  const state = Object.freeze({ leg: "support", launchPid: 123 });
  const proof = Object.freeze({
    leg: "support",
    ordinal: 12,
    rows: Object.freeze([]),
  });
  rejects(
    () =>
      receiptFaultCustodyReceipt(
        state,
        proof,
        Object.freeze({ kind: "legacy-disabled" }),
      ),
    "legacy receipt-fault custody",
  );
  return Object.freeze({
    legacyNumericSignalAuthority: false,
    negative: 1,
  });
}

async function runRubyCleanupModelSelfTests() {
  const keys = new Set();
  let suffixOrdinals = 0;
  for (const entry of RUBY_CLEANUP_SUFFIX_TABLE) {
    const key = `${entry.branchKey}:${entry.lastOrdinal}`;
    requireCondition(
      !keys.has(key) &&
        exactPrimitiveArray(
          entry.remaining,
          RUBY_BRANCH_ORDINALS[entry.branchKey].filter(
            (ordinal) => ordinal > entry.lastOrdinal,
          ),
        ) &&
        (entry.lastOrdinal > 12 ||
          (entry.remaining.includes(13) &&
            entry.remaining.includes(14))),
      "host.self_test",
      "Ruby cleanup suffix table is incomplete or duplicated",
      { entry },
    );
    keys.add(key);
    suffixOrdinals += entry.remaining.length;
  }
  requireCondition(
    RUBY_CLEANUP_SUFFIX_TABLE.length === 39 &&
      keys.size === 39 &&
      suffixOrdinals === 209 &&
      exactPrimitiveArray(
        RUBY_BRANCH_ORDINALS.ANCHORED,
        [12, 13, 14],
      ) &&
      RUBY_BRANCH_ORDINALS.SHORT.includes(12),
    "host.self_test",
    "Ruby cleanup suffix cardinality differs",
  );

  const firstFault = new HostAuthorityError(
    "host.kat_first_fault",
    "first",
  );
  const cleanupFixture = (
    entry,
    injectedOrdinal = undefined,
  ) => {
    const calls = [];
    const closeout = {
      receiptCloses: 0,
      roots: 0,
      fifo: 0,
      evidence: 0,
    };
    const state = {
      leg:
        entry.branchKey === "ANCHORED"
          ? "support"
          : "parent-loss",
      launchPid: 123,
      launched: {
        pid: 123,
        terminal: {
          promise: Promise.resolve(
            Object.freeze({
              code: 0,
              signal: null,
              error: null,
            }),
          ),
        },
      },
      deadline: {
        endsNs: MAX_U64,
        check() {},
        remainingMs() {
          return 1_000;
        },
      },
      roots: Object.freeze({}),
      branchKey: entry.branchKey,
      branch: RUBY_BRANCH_ORDINALS[entry.branchKey],
      consumed: new Set(
        RUBY_BRANCH_ORDINALS[entry.branchKey].filter(
          (ordinal) => ordinal <= entry.lastOrdinal,
        ),
      ),
      lastOrdinal: entry.lastOrdinal,
      proofRows: new Map(),
      proofs: new Map(),
      actions: new Map(),
      cleanupMode: false,
      parentReaped: true,
      directTerminal: Object.freeze({
        code: 0,
        signal: null,
        error: null,
      }),
      scope: Object.freeze({
        kind: "ReceiptAnchoredRubyScope",
        pgid: 123,
      }),
      roles: new Map([
        ["P", 123],
        ["C", 124],
      ]),
    };
    const capture = async (
      captureState,
      ordinal,
      _predicate,
      _label,
      action = "NONE",
    ) => {
      calls.push(Object.freeze({ ordinal, action }));
      const proof = Object.freeze({
        leg: captureState.leg,
        ordinal,
        rows: Object.freeze([]),
      });
      captureState.consumed.add(ordinal);
      captureState.lastOrdinal = ordinal;
      captureState.proofRows.set(
        ordinal,
        Object.freeze([]),
      );
      captureState.proofs.set(ordinal, proof);
      if (injectedOrdinal === ordinal) {
        throw new HostAuthorityError(
          "host.kat_capture_fault",
          "injected cleanup capture fault",
          { ordinal },
        );
      }
      return proof;
    };
    const proofOwner = {
      capture,
      async captureReceiptFaultCustody(
        captureState,
        predicate,
        label,
      ) {
        let deferredFault;
        try {
          await capture(
            captureState,
            12,
            predicate,
            label,
            "GROUP_KILL",
          );
        } catch (error) {
          deferredFault = error;
        }
        return Object.freeze({
          custody: Object.freeze({
            kind: "ReceiptFaultCustodyReceipt",
            leg: captureState.leg,
            ordinal: 12,
            signal: "SIGKILL",
            target: Object.freeze({
              kind: "group",
              pgid: captureState.launchPid,
            }),
            proofSha256: "a".repeat(64),
            monotonicNs: "1",
          }),
          deferredFault,
        });
      },
    };
    const output = Object.freeze({
      eof: true,
      bytes: 0,
      sha256:
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    });
    const owner = new RubyBranchCleanupOwner({
      state,
      support: true,
      proofOwner,
      receiptReader: {
        close() {
          closeout.receiptCloses += 1;
        },
      },
      receiptTransport: Object.freeze({
        bytes: 1,
        sha256: "b".repeat(64),
        records: Object.freeze([]),
      }),
      stdout: { promise: Promise.resolve(output) },
      stderr: { promise: Promise.resolve(output) },
      fifo: {},
      batch: {},
      evidence: {
        add() {
          closeout.evidence += 1;
        },
      },
      tombstones: {
        has() {
          return false;
        },
        add() {},
      },
    });
    owner.socketIdentities = [{}, {}];
    owner.retireRoots = () => {
      closeout.roots += 1;
      owner.rootsRetired = true;
    };
    owner.retireFifo = () => {
      closeout.fifo += 1;
      owner.fifoRetired = true;
      owner.receiptClosed = true;
    };
    return Object.freeze({ owner, calls, closeout });
  };

  let routeCases = 0;
  let injectedFaults = 0;
  for (const entry of RUBY_CLEANUP_SUFFIX_TABLE) {
    const normal = cleanupFixture(entry);
    await normal.owner.cleanup(firstFault);
    requireCondition(
      exactPrimitiveArray(
        normal.calls.map((call) => call.ordinal),
        entry.remaining,
      ) &&
        normal.closeout.receiptCloses === 1 &&
        normal.closeout.roots === 1 &&
        normal.closeout.fifo === 1 &&
        normal.closeout.evidence === 1,
      "host.self_test",
      "Ruby cleanup production suffix execution differs",
      { entry, calls: normal.calls, closeout: normal.closeout },
    );
    routeCases += 1;

    for (const ordinal of entry.remaining) {
      const injected = cleanupFixture(entry, ordinal);
      let caught;
      try {
        await injected.owner.cleanup(firstFault);
      } catch (error) {
        caught = error;
      }
      requireCondition(
        caught instanceof AggregateError &&
          exactPrimitiveArray(
            injected.calls.map((call) => call.ordinal),
            entry.remaining,
          ) &&
          injected.closeout.roots === 1 &&
          injected.closeout.fifo === 1 &&
          injected.closeout.evidence === 1,
        "host.self_test",
        "Ruby cleanup injected-fault suffix execution differs",
        { entry, ordinal, calls: injected.calls },
      );
      injectedFaults += 1;
    }
  }

  const dummy = {
    state: { leg: "support" },
    support: true,
    proofOwner: {},
    receiptReader: {},
    receiptTransport: {},
    stdout: {},
    stderr: {},
    fifo: {},
    batch: {},
    evidence: {},
    tombstones: {},
  };
  const owner = new RubyBranchCleanupOwner(dummy);
  let seen;
  owner.cleanup = async (fault) => {
    seen = fault;
  };
  let caught;
  try {
    await owner.run(async () => {
      throw firstFault;
    });
  } catch (error) {
    caught = error;
  }
  requireCondition(
    caught === firstFault && seen === firstFault,
    "host.self_test",
    "Ruby cleanup owner did not preserve its first typed fault",
  );

  const cleanupFault = new HostAuthorityError(
    "host.kat_cleanup_fault",
    "cleanup",
  );
  const aggregateOwner = new RubyBranchCleanupOwner(dummy);
  aggregateOwner.cleanup = async () => {
    throw cleanupFault;
  };
  caught = undefined;
  try {
    await aggregateOwner.run(async () => {
      throw firstFault;
    });
  } catch (error) {
    caught = error;
  }
  requireCondition(
    caught instanceof AggregateError &&
      caught.errors.length === 2 &&
      caught.errors[0] === firstFault &&
      caught.errors[1] === cleanupFault,
    "host.self_test",
    "Ruby cleanup owner did not preserve ordered dual faults",
  );

  let ordinaryCloseCalls = 0;
  const ordinaryOwner = new RubyBranchCleanupOwner({
    ...dummy,
    receiptReader: {
      close() {
        ordinaryCloseCalls += 1;
      },
    },
    fifo: {
      retire() {},
    },
    batch: Object.freeze({}),
    state: {
      ...dummy.state,
      deadline: Object.freeze({}),
    },
  });
  ordinaryOwner.retireFifo(false);
  ordinaryOwner.closeReceiptReader();
  requireCondition(
    ordinaryOwner.receiptClosed &&
      ordinaryCloseCalls === 0,
    "host.self_test",
    "ordinary FIFO retirement permitted a duplicate receipt close",
  );
  return Object.freeze({
    suffixes: RUBY_CLEANUP_SUFFIX_TABLE.length,
    suffixOrdinals,
    routeCases,
    injectedFaults: injectedFaults + 2,
    duplicateClosePrevented: true,
  });
}

async function runPureSelfTests() {
  const sourceDefects = sourceRecurrenceDefects();
  requireCondition(
    sourceDefects.length === 0 &&
      exactPrimitiveArray(sourceDefects, SOURCE_RECURRENCE_FAMILIES.slice(0, 0)),
    "host.self_test",
    "A0 source recurrence families remain open",
    { sourceDefects },
  );
  const receiptByteOwner = runReceiptByteOwnerSelfTests();
  const capacityLedger = runCapacityLedgerSelfTests();
  const productionCapacityRefusals =
    await runProductionCapacityRefusalSelfTests();
  const fstatDeadline = runFstatDeadlineSelfTests();
  const workerContract = runWorkerContractSelfTests();
  const custody = runCustodySelfTests();
  const nodeSupportBytes = Buffer.from(
    "SUCCESS|123|122|16777231|9007199254740993\n",
  );
  const nodeDenialBytes = Buffer.from(
    "DENIED|123|122|EPERM\n",
  );
  requireCondition(
    nodeOutput(nodeSupportBytes, true).ino ===
      "9007199254740993" &&
      nodeOutput(nodeDenialBytes, false).result === "DENIED",
    "host.self_test",
    "Node output positive KAT differs",
  );
  for (const [label, body, support, index] of [
    ["support prefix", nodeSupportBytes, true, 0],
    ["support digit", nodeSupportBytes, true, 8],
    ["denial prefix", nodeDenialBytes, false, 0],
    ["denial errno", nodeDenialBytes, false, 15],
  ]) {
    const hostile = Buffer.from(body);
    hostile[index] |= 0x80;
    rejects(
      () => nodeOutput(hostile, support),
      `Node output high-bit ${label}`,
    );
  }
  requireCondition(
    PACKET.version === "VSQ01S1_PHASE_A0_MAIN_V2" &&
      PACKET.design.bytes === 4_451 &&
      PACKET.design.lines === 138 &&
      PACKET.checker.bytes === 28_805 &&
      PACKET.checker.lines === 979 &&
      PACKET.manifestSha256 ===
        "a9e68dc484591ae9939c73056b595bc502fd36ed640d351f4f7fb8d71b1645c3" &&
      PACKET.pathNul.bytes === 42 &&
      PACKET.pathNul.sha256 ===
        "0f1adf61cf59eec4de1b9cfe9089016ae5fc8a2887132231a2b11ce34e7baaee",
    "host.self_test",
    "packet identity constants differ",
  );
  const captureSource =
    RubyProofOwner.prototype.capture.toString();
  const freshObservationSource =
    consumeFreshRubyObservation.toString();
  const proofIndex = freshObservationSource.indexOf(
    "const proof = Object.freeze",
  );
  const killIndex =
    freshObservationSource.indexOf("process.kill");
  const freshCallIndex = captureSource.indexOf(
    "consumeFreshRubyObservation(",
  );
  const fifoRetireIndex = captureSource.indexOf(
    "this.fifo.retire(",
  );
  const evidenceIndex = captureSource.indexOf("this.evidence.add");
  requireCondition(
    exactPrimitiveArray(RUBY_CAPTURE_ACTIONS, [
      "NONE",
      "GROUP_CONT",
      "GROUP_KILL",
      "GROUP_KILL_IF_PRESENT",
      "PARENT_KILL",
      "SURVIVOR_STOP",
      "SURVIVOR_CONT",
      "SURVIVOR_TERM",
      "SURVIVOR_KILL",
    ]) &&
      RubyProofOwner.prototype.signal === undefined &&
      proofIndex >= 0 &&
      killIndex > proofIndex &&
      freshCallIndex >= 0 &&
      fifoRetireIndex > freshCallIndex &&
      evidenceIndex > fifoRetireIndex &&
      !freshObservationSource
        .slice(proofIndex, killIndex)
        .includes("await") &&
      [
        "await",
        "checkCanary",
        "fstat",
        "readSync",
        "writeSync",
        "closeSync",
        "unlinkSync",
        "rmdirSync",
        "mkdirSync",
        "spawn",
        "evidence.add",
      ].every(
        (forbidden) =>
          !freshObservationSource
            .slice(proofIndex, killIndex)
            .includes(forbidden),
      ) &&
      !runWorker
        .toString()
        .includes("AbsoluteDeadline.fromNow"),
    "host.self_test",
    "fresh-signal or inherited-deadline source boundary differs",
  );
  const receiptReaderSource = rubyReceiptReader.toString();
  const receiptOwnerSource = readReceiptByteOwned.toString();
  const reservationIndex = receiptOwnerSource.indexOf(
    "counters.reservationAttempts += 1",
  );
  const readIndex = receiptOwnerSource.indexOf("count = readOne(");
  const postReturnIndex = receiptOwnerSource.indexOf(
    "postNowNs = sampleNow()",
  );
  const deadlineIndex = receiptOwnerSource.indexOf(
    "if (postNowNs < preNowNs || postNowNs > deadlineNs)",
  );
  const commitIndex = receiptOwnerSource.indexOf(
    "counters.committedBytes += 1",
  );
  const copyIndex = receiptOwnerSource.indexOf(
    "positiveByte = Buffer.from(scratch)",
  );
  const retainIndex = receiptReaderSource.indexOf("aggregateHash.update");
  const parseIndex = receiptReaderSource.indexOf(
    "parseRubyReceiptCandidate",
  );
  requireCondition(
    reservationIndex >= 0 &&
      readIndex > reservationIndex &&
      postReturnIndex > readIndex &&
      deadlineIndex > postReturnIndex &&
      commitIndex > deadlineIndex &&
      copyIndex > commitIndex &&
      retainIndex >= 0 &&
      parseIndex > retainIndex &&
      receiptReaderSource.includes(
        "sourceState.roles.length < 2",
      ) &&
      receiptReaderSource.includes(
        "counters.reads - countersBefore.reads === step.reads",
      ) &&
      receiptReaderSource.includes(
        "counters.committedBytes -",
      ),
    "host.self_test",
    "live receipt reserve/read/commit/retain/parse order differs",
  );
  requireCondition(
    NODE_LITERAL.length === 2_374 &&
      Buffer.byteLength(NODE_LITERAL) === 2_374 &&
      NODE_LITERAL.at(-1) === ";" &&
      sha256(Buffer.from(NODE_LITERAL)) ===
        "6bd0cfef095d9defdcb3b4d57f53d3c6301f2bd2a935c32b2701af87b7220e0d" &&
      RUBY_LITERAL.length === 5_089 &&
      Buffer.byteLength(RUBY_LITERAL) === 5_089 &&
      RUBY_LITERAL.at(-1) === "d" &&
      sha256(Buffer.from(RUBY_LITERAL)) ===
        "f8a2c939eae79931dccbc278c181abe75f3f9bf377c032989878f4be980b4f83",
    "host.self_test",
    "Node/Ruby literal identity differs",
  );
  requireCondition(
    FIXED_PINS.length === 16 &&
      FIXED_PINS.reduce((total, pin) => total + pin.bytes, 0) ===
        128_653_106 &&
      Object.values(PARTITION_CAPS).reduce(
        (total, bytes) => total + bytes,
        0,
      ) === EVIDENCE_MAX_BYTES,
    "host.self_test",
    "static/evidence arithmetic differs",
  );
  requireCondition(
    parseUnsignedBigInt(MAX_U64.toString()) === MAX_U64 &&
      parseUnsignedBigInt("9007199254740993") ===
        9_007_199_254_740_993n &&
      BigInt(Number(9_007_199_254_740_993n)) !==
        9_007_199_254_740_993n &&
      normalizeNativeDev(-688_741_003n) ===
        18_446_744_073_020_810_613n &&
      normalizeNativeDev(-1n) === MAX_U64,
    "host.self_test",
    "BigInt identity KAT differs",
  );
  for (const rejected of [
    "0",
    "18446744073709551616",
    "01",
    "-1",
    "+1",
    " 1",
    "1 ",
    "1_0",
  ]) {
    rejects(
      () => parseUnsignedBigInt(rejected),
      `unsigned identity ${rejected}`,
    );
  }
  for (const rejected of [
    9_223_372_036_854_775_808n,
    -9_223_372_036_854_775_809n,
  ]) {
    rejects(
      () => normalizeNativeDev(rejected),
      `native dev ${rejected}`,
    );
  }
  requireProtocolPeak(33);
  rejects(
    () => requireProtocolPeak(34),
    "protocol descriptor 34",
  );
  const tombstones = new Tombstones();
  requireCondition(
    !tombstones.has(2) &&
      !tombstones.has(99_999) &&
      tombstones.bytes.length === 12_500,
    "host.self_test",
    "tombstone boundary initialization differs",
  );
  tombstones.add(2, "KAT_MIN");
  tombstones.add(99_999, "KAT_MAX");
  requireCondition(
    tombstones.has(2) &&
      tombstones.has(99_999) &&
      tombstones.digest().count === 2 &&
      (tombstones.bytes[0] & 0b11) === 0,
    "host.self_test",
    "tombstone boundary projection differs",
  );
  const ancestor = "/private/tmp/marrow-vsq-a-12345678.ABCDEF";
  const fifoPaths = [];
  for (let batch = 0; batch < 36; batch += 1) {
    for (const name of ["stdout.fifo", "stderr.fifo"]) {
      fifoPaths.push(
        `${ancestor}/preflight/fifo/b${String(batch).padStart(3, "0")}/${name}`,
      );
    }
  }
  for (let batch = 36; batch < 40; batch += 1) {
    for (const name of [
      "receipt.fifo",
      "stdout.fifo",
      "stderr.fifo",
    ]) {
      fifoPaths.push(
        `${ancestor}/preflight/fifo/b${String(batch).padStart(3, "0")}/${name}`,
      );
    }
  }
  for (let batch = 40; batch < 42; batch += 1) {
    for (const name of ["stdout.fifo", "stderr.fifo"]) {
      fifoPaths.push(
        `${ancestor}/preflight/fifo/b${String(batch).padStart(3, "0")}/${name}`,
      );
    }
  }
  const longestArgv = [
    MKFIFO,
    "-m",
    "600",
    ...fifoPaths.slice(72, 75),
  ].reduce(
    (total, argument) => total + Buffer.byteLength(argument) + 1,
    0,
  );
  requireCondition(
    Buffer.byteLength(ancestor) === 41 &&
      fifoPaths.length === 88 &&
      fifoPaths.reduce(
        (total, path) => total + Buffer.byteLength(path),
        0,
      ) === 6_428 &&
      Math.max(
        ...fifoPaths.map((path) => Buffer.byteLength(path)),
      ) === 74 &&
      longestArgv === 246,
    "host.self_test",
    "FIFO topology arithmetic differs",
  );
  const nodePath = `${ancestor}/preflight/node-stream/control.sock`;
  const parentPath = `${ancestor}/preflight/ruby/parent/control.sock`;
  const childPath = `${ancestor}/preflight/ruby/child/control.sock`;
  const profiles = [
    sandboxProfile("node-support", [nodePath]),
    sandboxProfile("node-denial"),
    sandboxProfile("ruby-support", [parentPath, childPath]),
    sandboxProfile("ruby-denial"),
  ];
  requireCondition(
    new Set(profiles.map((profile) => profile.sha256)).size === 4 &&
      profiles.every(
        (profile) =>
          profile.bytes.length <= 4_096 &&
          profile.bytes.at(-1) === 0x0a,
      ),
    "host.self_test",
    "sandbox profile class identity differs",
  );
  const sampleReceipt =
    "P|S|123|122|123|123|16777234|9007199254740993\n";
  const parsedReceipt = parseRubyReceiptLine(sampleReceipt);
  requireCondition(
    parsedReceipt.role === "P" &&
      parsedReceipt.dev === "16777234" &&
      parsedReceipt.ino === "9007199254740993",
    "host.self_test",
    "Ruby receipt parser KAT differs",
  );
  rejects(
    () =>
      parseRubyReceiptLine(
        "P|D|123|122|123|123|bind|2\n",
      ),
    "Ruby denial errno",
  );
  const samplePs = Buffer.from(
    "  123   122   123     0   501   Ts   Wed Jul 23 01:02:03 2026   ruby            \n",
  );
  const parsedPs = parsePsRows(samplePs);
  requireCondition(
    parsedPs.length === 1 &&
      parsedPs[0].pid === 123 &&
      parsedPs[0].state === "Ts" &&
      parsedPs[0].sessObservedZero === "0" &&
      !Object.hasOwn(parsedPs[0], "sid"),
    "host.self_test",
    "ps parser KAT differs",
  );
  const psParser = runPsParserSelfTests();
  const rubyTopology = runRubyTopologySelfTests();
  const receiptTransport = runReceiptTransportSelfTests();
  const capacity = new Capacity("kat", 4);
  capacity.reserve(4);
  rejects(() => capacity.reserve(1), "capacity N+1");
  for (const maximum of [
    64,
    192,
    256,
    384,
    512,
    4_096,
    603_476,
  ]) {
    const boundary = new Capacity(`kat-${maximum}`, maximum);
    boundary.reserve(0);
    boundary.reserve(maximum);
    rejects(
      () => boundary.reserve(1),
      `capacity ${maximum + 1}`,
    );
  }
  const proofCaps = [5, 5, 13, 13];
  requireCondition(
    canonicalJson(proofCaps) === canonicalJson([5, 5, 13, 13]) &&
      proofCaps.reduce((total, count) => total + count, 0) === 36 &&
      rubyBranch("support").length === 6 &&
      rubyBranch("denial").length === 6 &&
      rubyBranch("one-receipt", "C").length === 13 &&
      rubyBranch("one-receipt", "P").length === 13,
    "host.self_test",
    "proof branch capacity differs",
  );
  return Object.freeze({
    packet: PACKET.version,
    nodeLiteralSha256: sha256(Buffer.from(NODE_LITERAL)),
    rubyLiteralSha256: sha256(Buffer.from(RUBY_LITERAL)),
    profiles: profiles.map((profile) => ({
      kind: profile.kind,
      bytes: profile.bytes.length,
      sha256: profile.sha256,
    })),
    fifo: {
      batches: 42,
      inodes: fifoPaths.length,
      pathBytes: 6_428,
      longestPath: 74,
      argvBytes: longestArgv,
    },
    descriptors: {
      taskFifo: 17,
      protocol: 33,
      refusal: 34,
    },
    receiptTransport,
    psParser,
    rubyTopology,
    receiptByteOwner,
    capacityLedger,
    productionCapacityRefusals,
    fstatDeadline,
    workerContract,
    custody,
  });
}

function runS2ReviewUnionStructuralSelfTests() {
  const ordinaryCaptureSource =
    S2CaptureReservationOwner.prototype.reserveOrdinary.toString();
  const replacementCaptureSource =
    S2CaptureReservationOwner.prototype.reserveReplacement.toString();
  const frameReaderSource = S2FrameReader.prototype.read.toString();
  const nodeStopSource = requireS2NodeStopShape.toString();
  const outerSource = runS2Supervisor.toString();
  const closeoutSource = closeS2ChildrenAfterFault.toString();
  const spawnSource = spawnExact.toString();
  const terminalSource = terminalLatch.toString();
  const faultKatSource = runS2FaultKat.toString();
  const resultSource = emitResult.toString();
  const captureOwnerSource = S2CaptureAttemptOwner.toString();
  const fifoSource = FifoManager.prototype.create.toString();
  const externalFifoSource =
    FifoManager.prototype.createExternal.toString();
  const protocolBootstrapSource =
    openS2ProtocolBootstrap.toString();
  const nodeStopDeliverySource =
    deliverS2NodeStop.toString();
  const captureReservedIndex = captureOwnerSource.indexOf(
    'lifecycle.enter("Reserved")',
  );
  const captureTryIndex = captureOwnerSource.indexOf(
    "try {",
    captureReservedIndex,
  );
  const captureDeadlineIndex = captureOwnerSource.indexOf(
    "proofDeadline = state.deadline.sub(",
    captureReservedIndex,
  );
  const captureCanaryIndex = captureOwnerSource.indexOf(
    "checkCanary(this.canary, proofDeadline)",
    captureReservedIndex,
  );
  const ordinaryReservationIndex =
    ordinaryCaptureSource.indexOf(
    "reserveS2CaptureAttempt(",
  );
  const nextBatchMutationIndex =
    ordinaryCaptureSource.indexOf(
    "this.nextBatch += 1",
  );
  const replacementReservationIndex =
    replacementCaptureSource.indexOf(
    "reserveS2CaptureAttempt(",
  );
  const replacementMutationIndex =
    replacementCaptureSource.indexOf(
    "this.replacementUsed = true",
  );
  const incomingReservationIndex = frameReaderSource.indexOf(
    "this.budget.reserveIncoming(chunk)",
  );
  const frameReservationIndex = frameReaderSource.indexOf(
    "this.budget.reserveFrame(",
  );
  const firstBufferMutationIndex = frameReaderSource.indexOf(
    "Buffer.concat(",
  );
  const adoptionIndex = spawnSource.indexOf(
    "onSpawn?.(provisional)",
  );
  const pidValidationIndex = spawnSource.indexOf(
    "Number.isInteger(child.pid)",
  );
  requireCondition(
    ordinaryReservationIndex >= 0 &&
      nextBatchMutationIndex > ordinaryReservationIndex &&
      replacementReservationIndex >= 0 &&
      replacementMutationIndex > replacementReservationIndex &&
      typeof S2TransportBudget.prototype.reserveIncoming ===
        "function" &&
      typeof S2TransportBudget.prototype.reserveFrame ===
        "function" &&
      incomingReservationIndex >= 0 &&
      firstBufferMutationIndex > incomingReservationIndex &&
      frameReservationIndex > incomingReservationIndex &&
      S2_NODE_STOP_KEYS.includes("secondaries") &&
      nodeStopSource.includes("decodeS2RawFact") &&
      typeof S2OuterCustody === "function" &&
      outerSource.includes("new S2OuterCustody(") &&
      outerSource.includes("custody.run(") &&
      outerSource.includes(
        "custodyOwner.bindSupervisor(provisional)",
      ) &&
      outerSource.includes(
        "custodyOwner.bindWorker(provisional)",
      ) &&
      closeoutSource.includes("cleanupStartNs") &&
      closeoutSource.includes(
        "!supervisorReader.ended",
      ) &&
      closeoutSource.includes(
        "!supervisorReader.failed",
      ) &&
      requireS2FaultCloseoutReserve(
        S2_FAULT_CLOSEOUT_MS.reserve,
      ).serialMs < S2_FAULT_CLOSEOUT_MS.reserve &&
      outerSource.includes(
        "nlink: evidenceFile.identity.nlink",
      ) &&
      outerSource.includes(
        "Buffer.from(evidenceFile.path)",
      ) &&
      adoptionIndex >= 0 &&
      pidValidationIndex > adoptionIndex &&
      fifoSource.includes(
        "onSpawn: (provisional) =>",
      ) &&
      externalFifoSource.includes(
        "onSpawn: (provisional) =>",
      ) &&
      captureReservedIndex >= 0 &&
      captureTryIndex > captureReservedIndex &&
      captureDeadlineIndex > captureTryIndex &&
      captureCanaryIndex > captureDeadlineIndex &&
      nodeStopDeliverySource.includes(
        "catch (writeFault)",
      ) &&
      nodeStopDeliverySource.includes(
        "firstFault: Object.freeze({",
      ) &&
      protocolBootstrapSource.indexOf(
        "requireProtocolPeak(S2_DESCRIPTOR_CAPACITY)",
      ) <
        protocolBootstrapSource.indexOf(
          "openS2NullSet(",
        ) &&
      !terminalSource.includes("outerDeadline") &&
      typeof runS2ProductionFaultSelfTests === "function" &&
      typeof runS2CaptureCapacitySelfTests === "function" &&
      faultKatSource.includes("runS2ProductionFaultSelfTests(") &&
      faultKatSource.includes("runS2CaptureCapacitySelfTests(") &&
      faultKatSource.includes("runS2NativeFaultSelfTests(") &&
      !faultKatSource.includes("rubyCleanup") &&
      Array.isArray(S2_DESCRIPTOR_PHASES) &&
      S2_DESCRIPTOR_PHASES.length === 13 &&
      resultSource.includes("deadline") &&
      resultSource.includes("writeAll(1, encoded, deadline)"),
    "host.self_test",
    "S2 whole-result review-union red remains open",
    {
      ordinaryReservationIndex,
      nextBatchMutationIndex,
      replacementReservationIndex,
      replacementMutationIndex,
      incomingReservationIndex,
      frameReservationIndex,
      firstBufferMutationIndex,
      nodeStopSecondaries:
        S2_NODE_STOP_KEYS.includes("secondaries"),
      outerCustody:
        typeof S2OuterCustody === "function",
      productionOwnerRecurrences:
        typeof runS2ProductionFaultSelfTests === "function",
      captureCapacityKat:
        typeof runS2CaptureCapacitySelfTests === "function",
      descriptorPhases:
        typeof S2_DESCRIPTOR_PHASES === "undefined"
          ? null
          : S2_DESCRIPTOR_PHASES.length,
      adoptionIndex,
      pidValidationIndex,
      terminalDeadlineLeak:
        terminalSource.includes("outerDeadline"),
      terminalReaderGuard:
        closeoutSource.includes(
          "!supervisorReader.ended",
        ) &&
        closeoutSource.includes(
          "!supervisorReader.failed",
        ),
      setupEvidenceIdentity:
        outerSource.includes(
          "nlink: evidenceFile.identity.nlink",
        ) &&
        outerSource.includes(
          "Buffer.from(evidenceFile.path)",
        ),
    },
  );
  return Object.freeze({
    captureReservationBeforeMutation: true,
    frameReservationBeforeRetention: true,
    nodeStopSecondaries: true,
    outerCustody: true,
    directChildAdoptionBeforeValidation: true,
    fifoChildAdoption: true,
    captureAttemptTotalOwner: true,
    nodeStopFirstFault: true,
    faultCloseoutReserve:
      S2_FAULT_CLOSEOUT_MS.reserve,
    nativeS2FaultKat: true,
    terminalReaderGuard: true,
    setupEvidenceIdentity: true,
    productionOwnerRecurrences: true,
    descriptorPhases: S2_DESCRIPTOR_PHASES.length,
    deadlineBoundResult: true,
  });
}

function runS2SourceStructuralSelfTests() {
  const receiptOwner = readReceiptByteOwned.toString();
  const receiptReader = rubyReceiptReader.toString();
  const readIndex = receiptOwner.indexOf("count = readOne(");
  const postIndex = receiptOwner.indexOf(
    "postNowNs = sampleNow()",
  );
  const guardIndex = receiptOwner.indexOf(
    "if (postNowNs < preNowNs || postNowNs > deadlineNs)",
  );
  const commitIndex = receiptOwner.indexOf(
    "counters.committedBytes += 1",
  );
  const copyIndex = receiptOwner.indexOf(
    "positiveByte = Buffer.from(scratch)",
  );
  const retainIndex = receiptReader.indexOf(
    "aggregateHash.update(positiveByte)",
  );
  const parseIndex = receiptReader.indexOf(
    "parseRubyReceiptCandidate(nextRecord)",
  );
  const captureSource = S2CaptureAttemptOwner.toString();
  const fifoSource = FifoManager.prototype.create.toString();
  const externalFifoSource =
    FifoManager.prototype.createExternal.toString();
  const protocolBootstrapSource =
    openS2ProtocolBootstrap.toString();
  const workerSource = runS2WorkerBody.toString();
  const outerSource = runS2Supervisor.toString();
  const rubyLegSource = runS2RubyLeg.toString();
  const transitionSource = rubyLegSource.slice(
    rubyLegSource.indexOf("const issueTransition"),
  );
  const relayIndex = transitionSource.indexOf(
    "exchange = await relay.command(",
  );
  const deferredCommitIndex = transitionSource.indexOf(
    "proofCommit = capture.commit()",
  );
  const workerCatchSource = runS2Worker.toString();
  const terminalStopIndex = workerCatchSource.indexOf(
    '"terminal.stop"',
  );
  const relayCloseIndex = workerCatchSource.indexOf(
    "relayToClose?.destroy()",
  );
  const closeEofIndex =
    CUSTODY_SUPERVISOR_LITERAL.indexOf(
      "owner.require_eof\n      owner.write([next_output.to_s,\"CLOSEOUT\"])",
    );
  requireCondition(
    readIndex >= 0 &&
      postIndex > readIndex &&
      guardIndex > postIndex &&
      commitIndex > guardIndex &&
      copyIndex > commitIndex &&
      retainIndex >= 0 &&
      parseIndex > retainIndex &&
      !receiptOwner
        .slice(postIndex, guardIndex)
        .includes("scratch") &&
      captureSource.indexOf(
        "this.reservationOwner.reserveOrdinary()",
      ) >= 0 &&
      captureSource.indexOf(
        "this.reservationOwner.reserveOrdinary()",
      ) <
        captureSource.indexOf("checkCanary(") &&
      captureSource.indexOf(
        'this.worker.capacity.reserve("proofs")',
      ) > captureSource.indexOf("parsePsRows(") &&
      captureSource.indexOf("nonthrowingOutcome(") <
        captureSource.indexOf("parsePsRows(") &&
      captureSource.indexOf(
        'this.tombstones.add(launched.pid, "S2_PS_DIRECT_REAP")',
      ) <
        captureSource.indexOf("this.fifo.retire(batch") &&
      relayIndex >= 0 &&
      deferredCommitIndex > relayIndex &&
      fifoSource.indexOf("reserveFifoBatch(") <
        fifoSource.indexOf("createDirectory(") &&
      externalFifoSource.indexOf("reserveFifoBatch(") <
        externalFifoSource.indexOf("createDirectory(") &&
      outerSource.indexOf("openS2ProtocolBootstrap(") >= 0 &&
      protocolBootstrapSource.indexOf(
        "reserveS2Protocol(",
      ) >= 0 &&
      protocolBootstrapSource.indexOf(
        "reserveS2Protocol(",
      ) <
        protocolBootstrapSource.indexOf(
          "openS2NullSet(",
        ) &&
      !workerSource.includes("process.kill(") &&
      !outerSource.includes("process.kill(") &&
      !rubyLegSource.includes("process.kill(") &&
      !runS2Worker.toString().includes(
        "AbsoluteDeadline.fromNow",
      ) &&
      terminalStopIndex >= 0 &&
      relayCloseIndex > terminalStopIndex &&
      runS2RelayServer
        .toString()
        .includes("deliverS2NodeStop(") &&
      closeEofIndex >= 0 &&
      CUSTODY_SUPERVISOR_LITERAL.includes(
        'stop!("PROTOCOL_STOP","terminal proof byte length") unless fields[4].to_i<=192',
      ) &&
      CUSTODY_SUPERVISOR_LITERAL.includes(
        "rescue SystemCallError\n        begin\n          report_write.syswrite(\"SETSID_STOP\\n\")",
      ) &&
      CUSTODY_SUPERVISOR_LITERAL.includes(
        'Process.kill("KILL",active[:pid])',
      ) &&
      CUSTODY_SUPERVISOR_LITERAL.includes(
        'Process.kill("KILL",-active[:pgid])',
      ) &&
      CUSTODY_SUPERVISOR_LITERAL.includes(
        "waited_pid,status=Process.wait2(active[:pid])",
      ) &&
      S2_TARGET_LITERAL.length === 6_191 &&
      sha256(Buffer.from(S2_TARGET_LITERAL)) ===
        "6dd83beadd9161cc603ef5a3882b13fb741631f95e82be354e704bb934b09c61",
    "host.self_test",
    "S2 source-order/incarnation recurrence boundary differs",
    {
      receiptOrder: [
        readIndex,
        postIndex,
        guardIndex,
        commitIndex,
        copyIndex,
        retainIndex,
        parseIndex,
      ],
      captureAdmissionIndex: captureSource.indexOf(
        "this.reservationOwner.reserveOrdinary()",
      ),
      captureCanaryIndex:
        captureSource.indexOf("checkCanary("),
      captureProofReservationIndex: captureSource.indexOf(
        'this.worker.capacity.reserve("proofs")',
      ),
      captureParseIndex:
        captureSource.indexOf("parsePsRows("),
      captureOutcomeIndex:
        captureSource.indexOf("nonthrowingOutcome("),
      captureTombstoneIndex: captureSource.indexOf(
        'this.tombstones.add(launched.pid, "S2_PS_DIRECT_REAP")',
      ),
      captureRetireIndex: captureSource.indexOf(
        "this.fifo.retire(batch",
      ),
      relayIndex,
      deferredCommitIndex,
      fifoReservationIndex:
        fifoSource.indexOf("reserveFifoBatch("),
      fifoCreateIndex:
        fifoSource.indexOf("createDirectory("),
      externalReservationIndex:
        externalFifoSource.indexOf("reserveFifoBatch("),
      externalCreateIndex:
        externalFifoSource.indexOf("createDirectory("),
      outerReservationIndex:
        protocolBootstrapSource.indexOf(
          "reserveS2Protocol(",
        ),
      outerOpenIndex:
        protocolBootstrapSource.indexOf(
          "openS2NullSet(",
        ),
      workerNumericSignal:
        workerSource.includes("process.kill("),
      outerNumericSignal:
        outerSource.includes("process.kill("),
      rubyNumericSignal:
        rubyLegSource.includes("process.kill("),
      workerDeadlineRemint:
        runS2Worker.toString().includes(
          "AbsoluteDeadline.fromNow",
        ),
      terminalStopIndex,
      relayCloseIndex,
      closeEofIndex,
      targetBytes: S2_TARGET_LITERAL.length,
      targetSha256: sha256(
        Buffer.from(S2_TARGET_LITERAL),
      ),
    },
  );
  const siblingFstatOwners = [
    streamRegular,
    streamOwner,
    readBoundedRegular,
    openEvidenceAppend,
    createEvidenceFile,
    openDevNull,
    fifoReceipt,
    openFifoEndpoints,
    verifyFifoEofAndClose,
    createCanary,
    checkCanary,
    retireCanary,
  ];
  requireCondition(
    siblingFstatOwners.every((owner) =>
      owner.toString().includes("fstatUnderDeadline(")
    ),
    "host.self_test",
    "S2 post-fstat deadline sibling family differs",
  );
  return Object.freeze({
    receiptOrder: [
      readIndex,
      postIndex,
      guardIndex,
      commitIndex,
      copyIndex,
      retainIndex,
      parseIndex,
    ],
    numericNodeRubySignals: 0,
    fstatOwners: siblingFstatOwners.length,
  });
}

async function runS2ProtocolSelfTests() {
  const tokens = [
    "1".repeat(32),
    "2".repeat(32),
    "3".repeat(32),
    "4".repeat(32),
  ];
  const readyRaw = Buffer.from(
    `0|READY|${tokens.join("|")}\n`,
    "ascii",
  );
  const ready = s2ParseReady(readyRaw);
  requireCondition(
    canonicalJson(ready.tokens) === canonicalJson(tokens),
    "host.self_test",
    "S2 READY token KAT differs",
  );
  for (const malformed of [
    Buffer.from(`0|READY|${tokens.slice(0, 3).join("|")}\n`),
    Buffer.from(
      `0|READY|${[
        tokens[0],
        tokens[0],
        tokens[2],
        tokens[3],
      ].join("|")}\n`,
    ),
    Buffer.concat([
      readyRaw.subarray(0, 2),
      Buffer.from([0xff]),
      readyRaw.subarray(3),
    ]),
  ]) {
    rejects(
      () => s2ParseReady(malformed),
      "S2 malformed READY",
    );
  }
  const frame = Buffer.from("X\n");
  const exact = new S2TransportBudget(
    "kat-exact",
    2,
    4,
  );
  exact.reserve(frame);
  exact.reserve(frame);
  rejects(() => exact.reserve(frame), "S2 frame N+1");
  requireCondition(
    exact.frames.used === 2 && exact.bytes.used === 4,
    "host.self_test",
    "S2 exact transport boundary differs",
  );
  const byteOverflow = new S2TransportBudget(
    "kat-byte",
    2,
    3,
  );
  byteOverflow.reserve(frame);
  rejects(
    () => byteOverflow.reserve(frame),
    "S2 aggregate byte N+1",
  );
  requireCondition(
    byteOverflow.frames.used === 1 &&
      byteOverflow.bytes.used === 2,
    "host.self_test",
    "S2 aggregate refusal mutated capacity",
  );
  const exactFrame = Buffer.concat([
    Buffer.alloc(S2_PROTOCOL.frameBytes - 1, 0x41),
    Buffer.from("\n"),
  ]);
  new S2TransportBudget(
    "kat-frame",
    1,
    S2_PROTOCOL.frameBytes,
  ).reserve(exactFrame);
  rejects(
    () =>
      new S2TransportBudget(
        "kat-overlong",
        1,
        S2_PROTOCOL.frameBytes + 1,
      ).reserve(
        Buffer.concat([exactFrame, Buffer.from("\n")]),
      ),
    "S2 frame byte 1025",
  );
  const deadline = AbsoluteDeadline.fromNow(
    "s2-frame-kat",
    10_000,
  );
  const splitReader = new S2FrameReader(
    Readable.from([
      Buffer.from("A"),
      Buffer.from("\nB\n"),
    ]),
    "split KAT",
    new S2TransportBudget("split KAT", 2, 4),
    deadline,
  );
  requireCondition(
    (await splitReader.read()).equals(Buffer.from("A\n")) &&
      (await splitReader.read()).equals(Buffer.from("B\n")) &&
      (await splitReader.read()) === null,
    "host.self_test",
    "S2 split frame/EOF KAT differs",
  );
  let partialNextCalls = 0;
  const partialReader = new S2FrameReader(
    Object.freeze({
      [Symbol.asyncIterator]() {
        return Object.freeze({
          next() {
            partialNextCalls += 1;
            return Promise.resolve(
              partialNextCalls === 1
                ? Object.freeze({
                    done: false,
                    value: Buffer.from("partial"),
                  })
                : Object.freeze({
                    done: true,
                    value: undefined,
                  }),
            );
          },
        });
      },
    }),
    "partial KAT",
    new S2TransportBudget("partial KAT", 1, 16),
    deadline,
  );
  await rejectsAsync(
    () => partialReader.read(),
    "S2 partial EOF",
  );
  await rejectsAsync(
    () => partialReader.read(),
    "S2 partial EOF retry",
  );
  await rejectsAsync(
    () => partialReader.requireEof(),
    "S2 partial EOF requireEof retry",
  );
  requireCondition(
    partialReader.failed && partialNextCalls === 2,
    "host.self_test",
    "S2 partial EOF did not latch across every reader entry",
    { partialNextCalls },
  );
  let timeoutNextCalls = 0;
  const timeoutReader = new S2FrameReader(
    Object.freeze({
      [Symbol.asyncIterator]() {
        return Object.freeze({
          next() {
            timeoutNextCalls += 1;
            return Promise.resolve(
              Object.freeze({
                done: false,
                value: Buffer.from("A\n"),
              }),
            );
          },
        });
      },
    }),
    "timeout KAT",
    new S2TransportBudget("timeout KAT", 1, 2),
    new AbsoluteDeadline(
      "expired S2 frame KAT",
      process.hrtime.bigint() - 1n,
    ),
  );
  await rejectsAsync(
    () => timeoutReader.read(),
    "S2 timeout",
  );
  await rejectsAsync(
    () => timeoutReader.read(),
    "S2 timeout retry",
  );
  requireCondition(
    timeoutReader.failed && timeoutNextCalls === 1,
    "host.self_test",
    "S2 timeout retried its terminal iterator operation",
    { timeoutNextCalls },
  );
  let trailingNextCalls = 0;
  const trailingReader = new S2FrameReader(
    Object.freeze({
      [Symbol.asyncIterator]() {
        return Object.freeze({
          next() {
            trailingNextCalls += 1;
            return Promise.resolve(
              Object.freeze({
                done: false,
                value: Buffer.from("A\n"),
              }),
            );
          },
        });
      },
    }),
    "trailing KAT",
    new S2TransportBudget("trailing KAT", 1, 2),
    deadline,
  );
  await rejectsAsync(
    () => trailingReader.requireEof(),
    "S2 trailing frame",
  );
  await rejectsAsync(
    () => trailingReader.read(),
    "S2 trailing frame retry",
  );
  requireCondition(
    trailingReader.failed && trailingNextCalls === 1,
    "host.self_test",
    "S2 trailing frame did not latch without retry",
    { trailingNextCalls },
  );
  const highBitBudget = new S2TransportBudget(
    "high-bit KAT",
    1,
    1,
  );
  const highBitReader = new S2FrameReader(
    Readable.from([Buffer.from([0xff])]),
    "high-bit KAT",
    highBitBudget,
    deadline,
  );
  await rejectsAsync(
    () => highBitReader.read(),
    "S2 high-bit frame input",
  );
  requireCondition(
    highBitBudget.frames.used === 0 &&
      highBitBudget.bytes.used === 1 &&
      highBitReader.faultSnapshot().raw.bytes === 1 &&
      highBitReader.failed,
    "host.self_test",
    "S2 high-bit charge-before-inspection KAT differs",
  );
  const frameRefusalBudget = new S2TransportBudget(
    "frame-refusal KAT",
    1,
    4,
  );
  const frameRefusalReader = new S2FrameReader(
    Readable.from([Buffer.from("A\nB\n")]),
    "frame-refusal KAT",
    frameRefusalBudget,
    deadline,
  );
  await rejectsAsync(
    () => frameRefusalReader.read(),
    "S2 coalesced frame N+1",
  );
  requireCondition(
    frameRefusalBudget.frames.used === 1 &&
      frameRefusalBudget.bytes.used === 4 &&
      frameRefusalReader.failed,
    "host.self_test",
    "S2 coalesced frame refusal accounting differs",
  );
  const aggregateRefusalBudget = new S2TransportBudget(
    "aggregate-refusal KAT",
    2,
    3,
  );
  const aggregateRefusalReader = new S2FrameReader(
    Readable.from([Buffer.from("A\nB\n")]),
    "aggregate-refusal KAT",
    aggregateRefusalBudget,
    deadline,
  );
  await rejectsAsync(
    () => aggregateRefusalReader.read(),
    "S2 incoming aggregate N+1",
  );
  requireCondition(
    aggregateRefusalBudget.frames.used === 0 &&
      aggregateRefusalBudget.bytes.used === 0,
    "host.self_test",
    "S2 incoming aggregate refusal mutated capacity",
  );
  const transitionCount = Object.values(
    S2_TRANSITION_PLAN,
  ).reduce(
    (total, transitions) =>
      total + transitions.length + 2,
    0,
  );
  requireCondition(
    transitionCount === 18 &&
      S2_LEGS.reduce(
        (total, leg) =>
          total + S2_PROOF_ORDINALS[leg].length,
        0,
      ) === 36 &&
      4 + 10 + 8 + 4 + 1 ===
        S2_PROTOCOL.supervisorInputFrames &&
      1 + 4 + 10 + 8 + 4 + 1 ===
        S2_PROTOCOL.supervisorOutputFrames &&
      S2_PROTOCOL.relayFrames === 28 &&
      S2_PROTOCOL.relayEnvelopes === 28,
    "host.self_test",
    "S2 command/proof/frame arithmetic differs",
  );
  return Object.freeze({
    tokens: tokens.length,
    transitions: transitionCount,
    proofs: 36,
    commands: 27,
    results: 28,
    transportFaults: 5,
    transportNoRetry: 3,
  });
}

function runS2CaptureCapacitySelfTests() {
  const capacity = new CapacityLedger();
  const owner = new S2CaptureReservationOwner(capacity);
  const admissions = [];
  for (let index = 0; index < 36; index += 1) {
    const admission = owner.reserveOrdinary();
    requireCondition(
      admission.batchIndex === index &&
        admission.replacement === false &&
        owner.begin(admission) === index + 1,
      "host.self_test",
      "S2 ordinary capture admission sequence differs",
      { index, admission, snapshot: owner.snapshot() },
    );
    admissions.push(admission);
  }
  const ordinaryBoundary = owner.snapshot();
  rejects(
    () => owner.reserveOrdinary(),
    "S2 ordinary capture batch 37",
  );
  requireCondition(
    canonicalJson(owner.snapshot()) ===
      canonicalJson(ordinaryBoundary),
    "host.self_test",
    "S2 ordinary N+1 mutated capture admission state",
  );
  const replacement = owner.reserveReplacement();
  requireCondition(
    replacement.batchIndex === 42 &&
      replacement.replacement === true &&
      owner.begin(replacement) === 37 &&
      owner.snapshot().capacity.reserved.captureAttempts === 37 &&
      owner.snapshot().capacity.reserved.psCaptures === 37,
    "host.self_test",
    "S2 sole replacement admission differs",
    { replacement, snapshot: owner.snapshot() },
  );
  const replacementBoundary = owner.snapshot();
  rejects(
    () => owner.reserveReplacement(),
    "S2 second cleanup replacement",
  );
  requireCondition(
    canonicalJson(owner.snapshot()) ===
      canonicalJson(replacementBoundary),
    "host.self_test",
    "S2 second replacement refusal mutated admission state",
  );

  const saturatedCapacity = new CapacityLedger({
    captureAttempts: LIMITS.captureAttempts,
    psCaptures: LIMITS.psCaptures,
  });
  const saturatedOwner =
    new S2CaptureReservationOwner(saturatedCapacity);
  const saturatedBefore = saturatedOwner.snapshot();
  rejects(
    () => saturatedOwner.reserveOrdinary(),
    "S2 capture reservation before batch mutation",
  );
  requireCondition(
    canonicalJson(saturatedOwner.snapshot()) ===
      canonicalJson(saturatedBefore),
    "host.self_test",
    "S2 saturated capture refusal mutated batch state",
  );

  const replacementCapacity = new CapacityLedger();
  const replacementOwner =
    new S2CaptureReservationOwner(replacementCapacity);
  replacementOwner.begin(
    replacementOwner.reserveOrdinary(),
  );
  for (
    let index = 1;
    index < LIMITS.captureAttempts;
    index += 1
  ) {
    reserveS2CaptureAttempt(replacementCapacity);
  }
  const replacementBefore = replacementOwner.snapshot();
  rejects(
    () => replacementOwner.reserveReplacement(),
    "S2 replacement reservation before used mutation",
  );
  requireCondition(
    canonicalJson(replacementOwner.snapshot()) ===
      canonicalJson(replacementBefore) &&
      replacementOwner.replacementUsed === false,
    "host.self_test",
    "S2 failed replacement reservation mutated affine state",
  );

  const ancestor =
    "/private/tmp/marrow-vsq-a-12345678.ABCDEF";
  const fifoCapacity = new CapacityLedger({
    directories: 27,
  });
  let fifoPathBytes = 0;
  let fifoInodes = 0;
  for (let batchIndex = 0; batchIndex <= 42; batchIndex += 1) {
    const names =
      batchIndex >= 36 && batchIndex <= 39
        ? ["receipt.fifo", "stdout.fifo", "stderr.fifo"]
        : ["stdout.fifo", "stderr.fifo"];
    const batchPath = `${ancestor}/preflight/fifo/b${String(
      batchIndex,
    ).padStart(3, "0")}`;
    const pathBytes = names.reduce(
      (total, name) =>
        total +
        Buffer.byteLength(`${batchPath}/${name}`),
      0,
    );
    const directory = reserveDirectory(fifoCapacity);
    const fifo = reserveFifoBatch(
      fifoCapacity,
      names.length,
      pathBytes,
    );
    fifoCapacity.complete(directory);
    fifoCapacity.complete(fifo.batch);
    fifoCapacity.complete(fifo.inodes);
    fifoCapacity.complete(fifo.pathBytes);
    fifoPathBytes += pathBytes;
    fifoInodes += names.length;
  }
  const fifoBoundary = fifoCapacity.snapshot();
  requireCondition(
    fifoBoundary.reserved.directories === 70 &&
      fifoBoundary.reserved.fifoBatches === 43 &&
      fifoBoundary.reserved.fifoInodes === 90 &&
      fifoBoundary.reserved.fifoPathBytes === 6_574 &&
      fifoPathBytes === 6_574 &&
      fifoInodes === 90,
    "host.self_test",
    "S2 exact 43-batch/90-inode production expansion differs",
    { fifoBoundary, fifoPathBytes, fifoInodes },
  );
  rejects(
    () =>
      reserveFifoBatch(fifoCapacity, 2, 146),
    "S2 FIFO batch 44",
  );
  requireCondition(
    canonicalJson(fifoCapacity.snapshot()) ===
      canonicalJson(fifoBoundary),
    "host.self_test",
    "S2 FIFO composite N+1 mutated capacity",
  );

  const success = new S2CaptureLifecycle();
  for (const state of [
    "Reserved",
    "AttemptMaterialized",
    "CaptureLaunched",
    "ProofInstalled",
    "Retired",
    "EvidenceCommitted",
  ]) {
    success.enter(state);
  }
  requireCondition(
    exactPrimitiveArray(success.snapshot(), [
      "Reserved",
      "AttemptMaterialized",
      "CaptureLaunched",
      "ProofInstalled",
      "Retired",
      "EvidenceCommitted",
    ]),
    "host.self_test",
    "S2 six-state successful capture trace differs",
  );
  const faultTraces = [
    ["Reserved", "RetiredNoObject", "EvidenceCommitted"],
    [
      "Reserved",
      "AttemptMaterialized",
      "Retired",
      "EvidenceCommitted",
    ],
    [
      "Reserved",
      "AttemptMaterialized",
      "CaptureLaunched",
      "Retired",
      "EvidenceCommitted",
    ],
    [
      "Reserved",
      "AttemptMaterialized",
      "CaptureLaunched",
      "ProofInstalled",
      "RetirementFault",
      "Retired",
    ],
    [
      "Reserved",
      "AttemptMaterialized",
      "CaptureLaunched",
      "ProofInstalled",
      "Retired",
      "EvidenceCommitFault",
    ],
  ];
  for (const trace of faultTraces) {
    const lifecycle = new S2CaptureLifecycle();
    for (const state of trace) lifecycle.enter(state);
    requireCondition(
      exactPrimitiveArray(lifecycle.snapshot(), trace),
      "host.self_test",
      "S2 capture fault trace differs",
      { trace },
    );
  }
  const invalid = new S2CaptureLifecycle();
  invalid.enter("Reserved");
  rejects(
    () => invalid.enter("ProofInstalled"),
    "S2 capture invalid transition",
  );
  return Object.freeze({
    ordinaryAdmissions: admissions.length,
    replacementAdmissions: 1,
    refusalCases: 4,
    fifoBatches: 43,
    fifoInodes: 90,
    fifoPathBytes: 6_574,
    successStates: success.snapshot().length,
    faultTraces: faultTraces.length,
  });
}

function s2CaptureKatState(deadline) {
  return {
    leg: "support",
    deadline,
    consumed: new Set(),
    lastOrdinal: 0,
    proofs: new Map(),
  };
}

function s2CaptureKatEvidence(fault = undefined) {
  const records = [];
  let attempts = 0;
  return Object.freeze({
    records,
    get attempts() {
      return attempts;
    },
    add(partition, kind, facts) {
      attempts += 1;
      if (fault !== undefined) throw fault;
      const record = Object.freeze({
        partition,
        kind,
        facts,
      });
      records.push(record);
      return record;
    },
  });
}

async function captureS2KatFault({
  deadline,
  canary,
  fifo,
  evidence,
  capacity = new CapacityLedger(),
}) {
  const worker = Object.freeze({
    capacity,
  });
  const owner = new S2CaptureAttemptOwner(
    worker,
    fifo,
    evidence,
    new Tombstones(),
    canary,
  );
  let caught;
  try {
    await owner.capture(
      s2CaptureKatState(deadline),
      1,
      () => true,
      "s2-production-fault-kat",
    );
  } catch (error) {
    caught = error;
  }
  requireCondition(
    caught instanceof HostAuthorityError,
    "host.self_test",
    "S2 production capture fault did not remain typed",
    { caught: safeError(caught) },
  );
  return Object.freeze({
    caught,
    snapshot: owner.reservationOwner.snapshot(),
  });
}

async function runS2CaptureNoObjectSelfTests() {
  const neverFifo = Object.freeze({
    async create() {
      fail(
        "host.self_test",
        "no-object capture KAT reached FIFO creation",
      );
    },
    retire() {
      fail(
        "host.self_test",
        "no-object capture KAT reached FIFO retirement",
      );
    },
  });
  const deadlineFault = new HostAuthorityError(
    "host.kat_capture_deadline",
    "injected capture deadline derivation fault",
  );
  const deadlineEvidence = s2CaptureKatEvidence();
  const deadlineResult = await captureS2KatFault({
    deadline: Object.freeze({
      check() {},
      sub() {
        throw deadlineFault;
      },
    }),
    canary: Object.freeze({}),
    fifo: neverFifo,
    evidence: deadlineEvidence,
  });
  requireCondition(
    deadlineResult.caught.code === deadlineFault.code &&
      deadlineResult.snapshot.attemptCount === 2 &&
      deadlineResult.snapshot.replacementUsed === true &&
      deadlineEvidence.records.length === 2 &&
      deadlineEvidence.records.every(
        (record) =>
          record.kind === "capture.attempt_fault" &&
          exactPrimitiveArray(record.facts.states, [
            "Reserved",
            "RetiredNoObject",
            "EvidenceCommitted",
          ]),
      ),
    "host.self_test",
    "deadline-fault capture did not retire/evidence both attempts",
    {
      caught: safeError(deadlineResult.caught),
      snapshot: deadlineResult.snapshot,
      records: deadlineEvidence.records,
    },
  );

  const canaryEvidence = s2CaptureKatEvidence();
  const canaryResult = await captureS2KatFault({
    deadline: AbsoluteDeadline.fromNow(
      "s2-capture-canary-kat",
      10_000,
    ),
    canary: Object.freeze({ reader: -1 }),
    fifo: neverFifo,
    evidence: canaryEvidence,
  });
  requireCondition(
    canaryResult.snapshot.attemptCount === 2 &&
      canaryResult.snapshot.replacementUsed === true &&
      canaryEvidence.records.length === 2 &&
      canaryEvidence.records.every(
        (record) =>
          exactPrimitiveArray(record.facts.states, [
            "Reserved",
            "RetiredNoObject",
            "EvidenceCommitted",
          ]),
      ),
    "host.self_test",
    "canary-fault capture did not retire/evidence both attempts",
    {
      caught: safeError(canaryResult.caught),
      snapshot: canaryResult.snapshot,
      records: canaryEvidence.records,
    },
  );

  const evidenceFault = new HostAuthorityError(
    "host.kat_capture_evidence",
    "injected capture evidence fault",
  );
  const failingEvidence = s2CaptureKatEvidence(evidenceFault);
  const evidenceResult = await captureS2KatFault({
    deadline: Object.freeze({
      check() {},
      sub() {
        throw deadlineFault;
      },
    }),
    canary: Object.freeze({}),
    fifo: neverFifo,
    evidence: failingEvidence,
  });
  requireCondition(
    evidenceResult.snapshot.attemptCount === 1 &&
      evidenceResult.snapshot.replacementUsed === false &&
      failingEvidence.attempts === 1 &&
      evidenceResult.caught.data?.replacementEligible === false &&
      evidenceResult.caught.data?.states?.includes(
        "EvidenceCommitFault",
      ),
    "host.self_test",
    "capture evidence fault did not close without replacement",
    {
      caught: safeError(evidenceResult.caught),
      snapshot: evidenceResult.snapshot,
      attempts: failingEvidence.attempts,
    },
  );
  const atBoundaryEvidence = s2CaptureKatEvidence();
  const atBoundary = await captureS2KatFault({
    deadline: AbsoluteDeadline.fromNow(
      "s2-capture-capacity-boundary-kat",
      10_000,
    ),
    canary: Object.freeze({ reader: -1 }),
    fifo: neverFifo,
    evidence: atBoundaryEvidence,
    capacity: new CapacityLedger({
      captureAttempts: LIMITS.captureAttempts - 1,
      psCaptures: LIMITS.psCaptures - 1,
    }),
  });
  requireCondition(
    atBoundary.snapshot.attemptCount === 1 &&
      atBoundary.snapshot.nextBatch === 1 &&
      atBoundary.snapshot.replacementUsed === false &&
      atBoundary.snapshot.capacity.reserved.captureAttempts ===
        LIMITS.captureAttempts &&
      atBoundary.snapshot.capacity.reserved.psCaptures ===
        LIMITS.psCaptures &&
      atBoundaryEvidence.records.length === 1,
    "host.self_test",
    "capture attempt 37/replacement 38 boundary differs",
    {
      caught: safeError(atBoundary.caught),
      snapshot: atBoundary.snapshot,
      records: atBoundaryEvidence.records,
    },
  );
  const overBoundaryEvidence = s2CaptureKatEvidence();
  const overBoundary = await captureS2KatFault({
    deadline: AbsoluteDeadline.fromNow(
      "s2-capture-capacity-refusal-kat",
      10_000,
    ),
    canary: Object.freeze({ reader: -1 }),
    fifo: neverFifo,
    evidence: overBoundaryEvidence,
    capacity: new CapacityLedger({
      captureAttempts: LIMITS.captureAttempts,
      psCaptures: LIMITS.psCaptures,
    }),
  });
  requireCondition(
    overBoundary.snapshot.attemptCount === 0 &&
      overBoundary.snapshot.nextBatch === 0 &&
      overBoundary.snapshot.replacementUsed === false &&
      overBoundaryEvidence.attempts === 0,
    "host.self_test",
    "capture attempt 38 mutated its production owner",
    {
      caught: safeError(overBoundary.caught),
      snapshot: overBoundary.snapshot,
      attempts: overBoundaryEvidence.attempts,
    },
  );
  return Object.freeze({
    deadlineAttempts: deadlineResult.snapshot.attemptCount,
    canaryAttempts: canaryResult.snapshot.attemptCount,
    evidenceFaultAttempts: evidenceResult.snapshot.attemptCount,
    replacementAttempts: 2,
    capacityN: atBoundary.snapshot.attemptCount,
    capacityNPlusOne: overBoundary.snapshot.attemptCount,
  });
}

function s2InjectedCaptureDeadline(base, seam, fault) {
  let outputChecks = 0;
  const authority = {
    endsNs: base.endsNs,
    atMost(label, endsNs) {
      return base.atMost(label, endsNs);
    },
    check(code = "host.deadline", message = undefined) {
      if (
        seam === "launch" &&
        message === "/dev/null fstat returned after deadline"
      ) {
        throw fault;
      }
      if (seam === "output" && code === "host.output_timeout") {
        outputChecks += 1;
        if (outputChecks === 3) throw fault;
      }
      base.check(code, message);
    },
    remainingMs() {
      return base.remainingMs();
    },
    remainingNs() {
      return base.remainingNs();
    },
    requireReserve(milliseconds, code = "host.deadline") {
      base.requireReserve(milliseconds, code);
    },
    sub(label, milliseconds) {
      if (/^s2-proof-/u.test(label)) return authority;
      return base.sub(label, milliseconds);
    },
    outputChecks() {
      return outputChecks;
    },
  };
  return Object.freeze(authority);
}

async function runS2ActiveCaptureFaultSelfTests(
  rootPath,
  rootReceipt,
  canary,
  deadline,
) {
  const results = [];
  for (const seam of ["launch", "output"]) {
    const fault = new HostAuthorityError(
      `host.kat_capture_${seam}`,
      `injected active capture ${seam} fault`,
    );
    const injectedDeadline = s2InjectedCaptureDeadline(
      deadline,
      seam,
      fault,
    );
    const records = [];
    const capacity = new CapacityLedger();
    const fifo = new FifoManager(
      {
        capacity,
        invocation: rootPath,
        fifoFacts: [],
      },
      rootReceipt,
      new Tombstones(),
      Object.freeze({
        add(partition, kind, facts) {
          const record = Object.freeze({
            partition,
            kind,
            facts,
          });
          records.push(record);
          return record;
        },
      }),
    );
    const owner = new S2CaptureAttemptOwner(
      Object.freeze({ capacity }),
      fifo,
      fifo.evidence,
      fifo.tombstones,
      canary,
    );
    const state = {
      ...s2CaptureKatState(injectedDeadline),
      pgid: process.pid,
      roots: Object.freeze({
        receipts: Object.freeze({
          home: rootReceipt,
          tmp: rootReceipt,
        }),
      }),
    };
    let caught;
    try {
      await owner.capture(
        state,
        1,
        () => true,
        `active-${seam}-fault`,
      );
    } catch (error) {
      caught = error;
    }
    requireCondition(
      caught instanceof HostAuthorityError &&
        fifo.used.has(0) &&
        absentNoFollow(join(rootPath, "b000")) &&
        absentNoFollow(join(rootPath, "b042")) &&
        records.some(
          (record) => record.kind === "fifo.batch",
        ) &&
        (seam !== "launch" || fifo.used.has(42)) &&
        (seam !== "output" ||
          injectedDeadline.outputChecks() >= 3),
      "host.self_test",
      `active capture ${seam} fault did not close its production objects`,
      {
        caught: safeError(caught),
        records: records.map((record) => record.kind),
        used: [...fifo.used],
        outputChecks: injectedDeadline.outputChecks(),
      },
    );
    results.push(
      Object.freeze({
        seam,
        code: caught.code,
        records: records.map((record) => record.kind),
        batches: Object.freeze([...fifo.used]),
        firstFault: caught.data?.firstFault ?? null,
        retained: caught.data?.retained === true,
        states: caught.data?.states ?? null,
      }),
    );
  }
  return Object.freeze(results);
}

async function runS2OuterCustodySelfTests() {
  const closeoutBudget = requireS2FaultCloseoutReserve(
    S2_FAULT_CLOSEOUT_MS.reserve,
  );
  rejects(
    () =>
      requireS2FaultCloseoutReserve(
        closeoutBudget.serialMs,
      ),
    "S2 closeout reserve equality",
  );
  requireCondition(
    requireS2FaultCloseoutReserve(
      closeoutBudget.serialMs + 1,
    ).reserveMs === closeoutBudget.serialMs + 1,
    "host.self_test",
    "S2 closeout reserve plus-one boundary differs",
  );
  const cleanDeadline = AbsoluteDeadline.fromNow(
    "s2-outer-custody-clean-kat",
    10_000,
  );
  const clean = new S2OuterCustody(
    cleanDeadline,
    async () => {
      throw new HostAuthorityError(
        "host.self_test",
        "clean outer custody invoked fault closeout",
      );
    },
  );
  requireCondition(
    (await clean.run(async () => "CLEAN")) === "CLEAN",
    "host.self_test",
    "S2 outer custody clean route differs",
  );

  const firstFault = new HostAuthorityError(
    "host.kat_outer_first",
    "outer first fault",
  );
  const cleanupFault = new HostAuthorityError(
    "host.kat_outer_cleanup",
    "outer cleanup secondary",
  );
  const terminal = {
    current() {
      return null;
    },
  };
  const child = Object.freeze({ terminal });
  let closeCalls = 0;
  const faultOwner = new S2OuterCustody(
    AbsoluteDeadline.fromNow(
      "s2-outer-custody-fault-kat",
      10_000,
    ),
    async (
      supervisor,
      reader,
      worker,
      deadline,
      cleanupStartNs,
    ) => {
      closeCalls += 1;
      requireCondition(
        supervisor === child &&
          reader === undefined &&
          worker === child &&
          deadline instanceof AbsoluteDeadline &&
          typeof cleanupStartNs === "bigint",
        "host.self_test",
        "S2 outer custody closeout arguments differ",
      );
      return Object.freeze({
        workerTerminal: Object.freeze({
          code: 0,
          signal: null,
          error: null,
        }),
        supervisorTerminal: Object.freeze({
          code: 0,
          signal: null,
          error: null,
        }),
        retainedSupervisorFrames: Object.freeze([]),
        secondary: Object.freeze([cleanupFault]),
      });
    },
  );
  faultOwner.bindSupervisor(child);
  faultOwner.bindWorker(child);
  let caught;
  try {
    await faultOwner.run(async () => {
      throw firstFault;
    });
  } catch (error) {
    caught = error;
  }
  requireCondition(
    caught instanceof HostAuthorityError &&
      caught.code === firstFault.code &&
      caught.data?.firstFault?.code === firstFault.code &&
      caught.data?.secondaryFaults?.length === 1 &&
      caught.data.secondaryFaults[0].code ===
        cleanupFault.code &&
      closeCalls === 1,
    "host.self_test",
    "S2 outer custody first/secondary fault ordering differs",
    { caught: safeError(caught), closeCalls },
  );
  return Object.freeze({
    cleanRoutes: 1,
    firstFaultRoutes: 1,
    secondaryFaults: 1,
    closeoutBudget,
  });
}

async function runS2NodeStopFirstFaultSelfTest() {
  const stopFrame = Buffer.from(
    "1|DEADLINE_STOP|DEADLINE_STOP||\n",
    "ascii",
  );
  const supervisorTerminal = Object.freeze({
    code: 77,
    error: null,
    signal: null,
  });
  const supervisor = Object.freeze({
    child: Object.freeze({
      stdin: Object.freeze({
        end() {},
      }),
    }),
    terminal: Object.freeze({
      current() {
        return supervisorTerminal;
      },
    }),
  });
  const supervisorReader = Object.freeze({
    ended: true,
    failed: false,
    faultSnapshot() {
      return Object.freeze({
        raw: s2RawFact(Buffer.alloc(0)),
      });
    },
  });
  const writeFault = new HostAuthorityError(
    "host.kat_node_stop_write",
    "injected NODE_STOP envelope write fault",
  );
  const workerStream = Object.freeze({
    write() {
      throw writeFault;
    },
  });
  let caught;
  try {
    await deliverS2NodeStop({
      supervisor,
      supervisorReader,
      workerStream,
      workerReader: Object.freeze({}),
      workerWriterBudget: new S2TransportBudget(
        "NODE_STOP first-fault KAT",
        1,
        S2_PROTOCOL.resultEnvelopeBytes,
        S2_PROTOCOL.resultEnvelopeBytes,
      ),
      rawIntent: Buffer.from("{}\n", "ascii"),
      sequence: 0,
      supervisorResult: stopFrame,
      deadline: AbsoluteDeadline.fromNow(
        "s2-node-stop-first-fault-kat",
        10_000,
      ),
    });
  } catch (error) {
    caught = error;
  }
  let joinedCaught;
  try {
    selectS2ConcurrentOutcomes(
      Object.freeze({
        error: caught,
        kind: "FAULT",
        ordinal: 0,
        role: "relay",
      }),
      Object.freeze({
        kind: "VALUE",
        ordinal: 1,
        role: "worker",
        value: Object.freeze({
          terminal: Object.freeze({
            code: 0,
            error: null,
            signal: null,
          }),
        }),
      }),
    );
  } catch (error) {
    joinedCaught = error;
  }
  const concurrentFaultOrder = async (firstRole) => {
    let relayReject;
    let workerReject;
    const relayPending = new Promise((_resolve, reject) => {
      relayReject = reject;
    });
    const workerPending = new Promise((_resolve, reject) => {
      workerReject = reject;
    });
    const settlement = new S2ConcurrentSettlementLatch();
    const relayCaptured = settlement.capture(
      "relay",
      relayPending,
    );
    const workerCaptured = settlement.capture(
      "worker",
      workerPending,
    );
    const relayFault = new HostAuthorityError(
      "host.kat_relay_concurrent",
      "injected relay concurrent fault",
    );
    const workerFault = new HostAuthorityError(
      "host.kat_worker_concurrent",
      "injected worker concurrent fault",
    );
    if (firstRole === "relay") {
      relayReject(relayFault);
      await Promise.resolve();
      workerReject(workerFault);
    } else {
      workerReject(workerFault);
      await Promise.resolve();
      relayReject(relayFault);
    }
    const [relaySettled, workerSettled] =
      await Promise.all([relayCaptured, workerCaptured]);
    let selected;
    try {
      selectS2ConcurrentOutcomes(
        relaySettled,
        workerSettled,
      );
    } catch (error) {
      selected = error;
    }
    return Object.freeze({
      firstCode:
        firstRole === "relay"
          ? relayFault.code
          : workerFault.code,
      secondCode:
        firstRole === "relay"
          ? workerFault.code
          : relayFault.code,
      selected,
    });
  };
  const relayFirst =
    await concurrentFaultOrder("relay");
  const workerFirst =
    await concurrentFaultOrder("worker");
  requireCondition(
    caught instanceof HostAuthorityError &&
      caught.code === "DEADLINE_STOP" &&
      caught.data?.firstFault?.code === "DEADLINE_STOP" &&
      caught.data?.firstFault?.kind === "DEADLINE_STOP" &&
      caught.data?.secondaryFaults?.at(-1)?.code ===
        writeFault.code &&
      joinedCaught instanceof HostAuthorityError &&
      joinedCaught.code === "DEADLINE_STOP" &&
      joinedCaught.data?.firstFault?.code ===
        "DEADLINE_STOP" &&
      joinedCaught.data?.secondaryFaults?.at(-1)?.code ===
        writeFault.code &&
      joinedCaught.data?.retained === true,
    "host.self_test",
    "NODE_STOP join replaced its selected first fault",
    {
      caught: safeError(caught),
      joinedCaught: safeError(joinedCaught),
    },
  );
  requireCondition(
    [relayFirst, workerFirst].every(
      (result) =>
        result.selected instanceof HostAuthorityError &&
        result.selected.code === result.firstCode &&
        result.selected.data?.secondaryFaults?.at(-1)
          ?.code === result.secondCode,
    ),
    "host.self_test",
    "S2 concurrent join reordered its first settled fault",
    {
      relayFirst: safeError(relayFirst.selected),
      workerFirst: safeError(workerFirst.selected),
    },
  );
  return Object.freeze({
    firstFault: joinedCaught.code,
    secondaryFault:
      joinedCaught.data.secondaryFaults.at(-1).code,
  });
}

function runS2ProofInstalledEvidenceFaultSelfTest() {
  const lifecycle = new S2CaptureLifecycle();
  for (const state of [
    "Reserved",
    "AttemptMaterialized",
    "CaptureLaunched",
    "ProofInstalled",
    "Retired",
  ]) {
    lifecycle.enter(state);
  }
  const evidenceFault = new HostAuthorityError(
    "host.kat_capture_evidence",
    "injected proof-installed evidence fault",
  );
  const evidence = Object.freeze({
    add() {
      throw evidenceFault;
    },
  });
  let inner;
  try {
    evidence.add(
      "ps",
      "ruby.proof",
      Object.freeze({ proofInstalled: true }),
    );
  } catch (error) {
    lifecycle.enter("EvidenceCommitFault");
    inner = new HostAuthorityError(
      "host.capture_evidence",
      "S2 proof evidence commit failed",
      {
        firstFault: safeError(error),
        secondaryFaults: [],
        states: lifecycle.snapshot(),
        replacementEligible: false,
        retained: true,
      },
    );
  }
  const outward = s2CaptureTypedFault(
    inner,
    [],
    lifecycle,
    false,
  );
  requireCondition(
    outward.code === "host.capture_evidence" &&
      outward.data?.retained === true &&
      outward.data?.replacementEligible === false &&
      outward.data?.states?.at(-1) ===
        "EvidenceCommitFault",
    "host.self_test",
    "proof-installed evidence fault lost retained status",
    { outward: safeError(outward) },
  );
  return Object.freeze({
    code: outward.code,
    retained: outward.data.retained,
    states: outward.data.states,
  });
}

async function runS2ProductionFaultSelfTests() {
  const outer = await runS2OuterCustodySelfTests();
  const captureNoObject =
    await runS2CaptureNoObjectSelfTests();
  const nodeStopFirstFault =
    await runS2NodeStopFirstFaultSelfTest();
  const proofInstalledEvidence =
    runS2ProofInstalledEvidenceFaultSelfTest();
  const secondary = s2SecondaryFacts([
    new HostAuthorityError(
      "host.kat_secondary",
      "typed secondary",
    ),
  ]);
  validateS2SecondaryFacts(
    secondary,
    "production fault KAT",
  );
  const descriptor = s2DescriptorModel();
  requireProtocolPeak(
    descriptor.phases[1].slots,
    descriptor.phases[1].phase,
  );
  requireProtocolPeak(
    descriptor.phases[4].slots,
    descriptor.phases[4].phase,
  );
  return Object.freeze({
    outer,
    captureNoObject,
    nodeStopFirstFault,
    proofInstalledEvidence,
    nodeStopSecondaryFacts: secondary.length,
    descriptorPhases: descriptor.phases.length,
    descriptorCapacity: descriptor.capacity,
  });
}

async function runS2NativeFaultSelfTests(outerDeadline) {
  const deadline = outerDeadline.sub(
    "s2-native-fault-kat",
    10_000,
  );
  const rootPath = mkdtempSync(
    `/private/tmp/marrow-vsq-a-kat-${randomToken().slice(0, 8)}.`,
  );
  chmodSync(rootPath, 0o700);
  chownSync(rootPath, HOST_UID, HOST_GID);
  const rootReceipt = rootFact(rootPath, "s2-native-fault-kat");
  let canary;
  let fifoCapacityRoot;
  let firstFault;
  const cleanupFaults = [];
  try {
    canary = createCanary(
      { capacity: new CapacityLedger() },
      rootReceipt,
      deadline,
    );
    const activeCapture =
      await runS2ActiveCaptureFaultSelfTests(
        rootPath,
        rootReceipt,
        canary,
        deadline,
      );
    const fifoFacts = [];
    const tombstoneRecords = [];
    let tombstoneQueries = 0;
    const fifo = new FifoManager(
      {
        capacity: new CapacityLedger(),
        invocation: rootPath,
        fifoFacts,
      },
      rootReceipt,
      Object.freeze({
        has() {
          tombstoneQueries += 1;
          return tombstoneQueries === 1;
        },
        add(pid, reason) {
          tombstoneRecords.push(
            Object.freeze({ pid, reason }),
          );
        },
      }),
      Object.freeze({
        add() {
          fail(
            "host.self_test",
            "post-spawn validation KAT committed clean FIFO evidence",
          );
        },
      }),
    );
    let fifoFault;
    try {
      await fifo.create(
        0,
        ["stdout.fifo", "stderr.fifo"],
        deadline,
      );
    } catch (error) {
      fifoFault = error;
    }
    requireCondition(
      fifoFault instanceof HostAuthorityError &&
        fifoFault.code === "host.spawn" &&
        tombstoneQueries === 2 &&
        tombstoneRecords.length === 1 &&
        tombstoneRecords[0].reason ===
          "FIFO_FAULT_DIRECT_REAP" &&
        fifo.used.has(0) &&
        fifoFacts.length === 0 &&
        absentNoFollow(join(rootPath, "b000")),
      "host.self_test",
      "FIFO post-spawn validation fault lost direct-child custody",
      {
        fifoFault: safeError(fifoFault),
        tombstoneQueries,
        tombstoneRecords,
        fifoFacts,
      },
    );

    const fifoCapacity = new CapacityLedger({
      directories: 68,
      fifoBatches: LIMITS.fifoBatches - 1,
      fifoInodes: LIMITS.fifoInodes - 2,
      fifoPathBytes: LIMITS.fifoPathBytes - 146,
    });
    const fifoCapacityPath = join(rootPath, "capacityxx");
    requireCondition(
      Buffer.byteLength(fifoCapacityPath) === 56,
      "host.self_test",
      "FIFO capacity KAT root length differs",
      { bytes: Buffer.byteLength(fifoCapacityPath) },
    );
    fifoCapacityRoot = createDirectory(
      fifoCapacityPath,
      "s2-fifo-capacity-kat",
      fifoCapacity,
    );
    const boundaryFacts = [];
    const boundaryFifo = new FifoManager(
      {
        capacity: fifoCapacity,
        invocation: rootPath,
        fifoFacts: boundaryFacts,
      },
      fifoCapacityRoot,
      new Tombstones(),
      Object.freeze({
        add() {
          return Object.freeze({ kind: "fifo.batch" });
        },
      }),
    );
    const boundaryBatch = await boundaryFifo.create(
      42,
      ["stdout.fifo", "stderr.fifo"],
      deadline,
    );
    closeHandoff(boundaryBatch);
    boundaryFifo.retire(boundaryBatch, deadline);
    const boundarySnapshot = fifoCapacity.snapshot();
    requireCondition(
      boundarySnapshot.reserved.directories ===
          LIMITS.directories &&
        boundarySnapshot.completed.directories ===
          LIMITS.directories &&
        boundarySnapshot.reserved.fifoBatches ===
          LIMITS.fifoBatches &&
        boundarySnapshot.completed.fifoBatches ===
          LIMITS.fifoBatches &&
        boundarySnapshot.reserved.fifoInodes ===
          LIMITS.fifoInodes &&
        boundarySnapshot.completed.fifoInodes ===
          LIMITS.fifoInodes &&
        boundarySnapshot.reserved.fifoPathBytes ===
          LIMITS.fifoPathBytes &&
        boundarySnapshot.completed.fifoPathBytes ===
          LIMITS.fifoPathBytes &&
        boundaryFacts.length === 2 &&
        boundaryFifo.used.has(42),
      "host.self_test",
      "FIFO 43/90/6574 production boundary differs",
      {
        boundarySnapshot,
        facts: boundaryFacts.length,
        used: [...boundaryFifo.used],
      },
    );
    const refusalFifo = new FifoManager(
      {
        capacity: fifoCapacity,
        invocation: rootPath,
        fifoFacts: [],
      },
      fifoCapacityRoot,
      new Tombstones(),
      Object.freeze({}),
    );
    let fifoCapacityFault;
    try {
      await refusalFifo.create(
        0,
        ["stdout.fifo", "stderr.fifo"],
        deadline,
      );
    } catch (error) {
      fifoCapacityFault = error;
    }
    requireCondition(
      fifoCapacityFault instanceof HostAuthorityError &&
        refusalFifo.used.size === 0 &&
        canonicalJson(fifoCapacity.snapshot()) ===
          canonicalJson(boundarySnapshot) &&
        absentNoFollow(join(fifoCapacityPath, "b000")),
      "host.self_test",
      "FIFO N+1 production refusal mutated or created",
      {
        fault: safeError(fifoCapacityFault),
        used: [...refusalFifo.used],
      },
    );
    removeDirectory(fifoCapacityPath, fifoCapacityRoot);
    fifoCapacityRoot = undefined;

    const tracked = [];
    const custody = Object.freeze({
      trackDescriptor(entry) {
        tracked.push(entry);
      },
    });
    const protocol = openS2ProtocolBootstrap(
      { capacity: new CapacityLedger() },
      custody,
      deadline,
    );
    closeParentDescriptors([protocol.supervisorNull.fd]);
    completeS2ProtocolReservations(
      capacityReservationOwners.get(
        protocol.reservations.supervisor,
      ),
      protocol.reservations,
    );
    let descriptorFault;
    let refusedTracks = 0;
    try {
      openS2ProtocolBootstrap(
        {
          capacity: new CapacityLedger({
            descriptorSlots: 1,
          }),
        },
        Object.freeze({
          trackDescriptor() {
            refusedTracks += 1;
          },
        }),
        deadline,
      );
    } catch (error) {
      descriptorFault = error;
    }
    requireCondition(
      tracked.length === 1 &&
        descriptorFault instanceof HostAuthorityError &&
        refusedTracks === 0,
      "host.self_test",
      "descriptor 59/60 production bootstrap boundary differs",
      {
        tracked: tracked.length,
        refusedTracks,
        descriptorFault: safeError(descriptorFault),
      },
    );
    return Object.freeze({
      activeCapture,
      fifoAdoption: Object.freeze({
        tombstones: tombstoneRecords.length,
        queries: tombstoneQueries,
      }),
      fifoCapacity: Object.freeze({
        batches: LIMITS.fifoBatches,
        inodes: LIMITS.fifoInodes,
        pathBytes: LIMITS.fifoPathBytes,
        refusalBeforeCreate: true,
      }),
      descriptorBoundary: Object.freeze({
        success: S2_DESCRIPTOR_CAPACITY,
        refusal: S2_DESCRIPTOR_CAPACITY + 1,
      }),
    });
  } catch (error) {
    firstFault = error;
    throw error;
  } finally {
    if (canary !== undefined) {
      try {
        retireCanary(canary, deadline);
        canary = undefined;
      } catch (error) {
        cleanupFaults.push(error);
      }
    }
    if (fifoCapacityRoot !== undefined) {
      try {
        if (
          sameRoot(fifoCapacityRoot.path, fifoCapacityRoot) &&
          readdirSync(fifoCapacityRoot.path).length === 0
        ) {
          removeDirectory(
            fifoCapacityRoot.path,
            fifoCapacityRoot,
          );
          fifoCapacityRoot = undefined;
        }
      } catch (error) {
        cleanupFaults.push(error);
      }
    }
    try {
      if (
        sameRoot(rootPath, rootReceipt) &&
        readdirSync(rootPath).length === 0
      ) {
        removeDirectory(rootPath, rootReceipt);
      } else {
        cleanupFaults.push(
          new HostAuthorityError(
            "host.self_test",
            "native fault KAT root retained an object",
            {
              pathHash: rootReceipt.pathHash,
              entries: readdirSync(rootPath),
            },
          ),
        );
      }
    } catch (error) {
      cleanupFaults.push(error);
    }
    if (cleanupFaults.length > 0) {
      throw aggregate(firstFault, cleanupFaults);
    }
  }
}

function runS2FaultProtocolSelfTests() {
  const report = Buffer.from("SETSID_STOP\n", "ascii");
  const release = Buffer.alloc(0);
  const startupDetails = Buffer.from(
    [
      report.toString("base64"),
      String(report.length),
      sha256(report),
      "1",
      release.toString("base64"),
      "0",
      sha256(release),
      "0",
      "",
    ].join("|"),
    "ascii",
  );
  const stopFrame = Buffer.from(
    `1|COMMAND_STOP|COMMAND_STOP|${startupDetails.toString("base64")}|\n`,
    "ascii",
  );
  const parsedStop = parseS2SupervisorStop(stopFrame);
  requireCondition(
    parsedStop.kind === "COMMAND_STOP" &&
      parsedStop.startup.reportEof === true &&
      parsedStop.startup.releaseEof === false &&
      decodeS2RawFact(
        parsedStop.startup.report,
        "STOP KAT report",
        S2_PROTOCOL.startupReportBytes,
      ).equals(report),
    "host.self_test",
    "closed supervisor STOP KAT differs",
  );
  const nodeStop = Object.freeze({
    schema: 1,
    sequence: 1,
    kind: "NODE_STOP",
    stopKind: "COMMAND_STOP",
    faultCode: "COMMAND_STOP",
    workerIntentSha256: "a".repeat(64),
    supervisorResult: s2RawFact(stopFrame),
    readerFault: s2RawFact(Buffer.alloc(0)),
    resultEof: true,
    secondaries: Object.freeze([]),
    supervisorTerminal: Object.freeze({
      code: 77,
      error: null,
      signal: null,
    }),
  });
  requireS2NodeStopShape(nodeStop, "KAT");
  for (const malformed of [
    Object.freeze({ ...nodeStop, extra: true }),
    Object.freeze({
      ...nodeStop,
      supervisorTerminal: Object.freeze({
        ...nodeStop.supervisorTerminal,
        extra: true,
      }),
    }),
    Object.freeze({
      ...nodeStop,
      supervisorResult: s2RawFact(
        Buffer.from("1|COMMAND_STOP|COMMAND_STOP||\ntrailing"),
      ),
    }),
  ]) {
    rejects(
      () => requireS2NodeStopShape(malformed, "KAT malformed"),
      "NODE_STOP closed schema",
    );
  }
  for (const malformed of [
    Buffer.from("1|COMMAND_STOP|COMMAND_STOP|\n"),
    Buffer.from("1|COMMAND_STOP|COMMAND_STOP|||extra\n"),
    Buffer.from("1|PROTOCOL_STOP|COMMAND_STOP||\n"),
  ]) {
    rejects(
      () => parseS2SupervisorStop(malformed),
      "supervisor STOP closed schema",
    );
  }
  const rootPlan = expectedS2RootCreationPlan(
    "/private/tmp/marrow-vsq-a-12345678.ABCDEF",
  );
  const rootInventory = rootPlan.map((entry, index) =>
    Object.freeze({
      dev: "16777231",
      gid: HOST_GID,
      ino: String(10_000 + index),
      mode: 0o700,
      nlink: 2,
      pathHash: entry.pathHash,
      role: entry.role,
      type: "directory",
      uid: HOST_UID,
    })
  );
  validateS2RootInventory(
    rootInventory,
    "/private/tmp/marrow-vsq-a-12345678.ABCDEF",
  );
  const rootMutation = rootInventory.map((entry) => ({ ...entry }));
  rootMutation[68].role = "fifo-b999";
  rejects(
    () =>
      validateS2RootInventory(
        rootMutation,
        "/private/tmp/marrow-vsq-a-12345678.ABCDEF",
      ),
    "root inventory creation order",
  );
  return Object.freeze({
    stopSchemas: 2,
    stopNegatives: 6,
    rootReceipts: rootPlan.length,
  });
}

function runS2CapacitySelfTests() {
  const ledger = runCapacityLedgerSelfTests();
  const unchangedAfterRefusal = (
    owner,
    reserveOperation,
  ) => {
    const capacity = new CapacityLedger({
      [owner]: CAPACITY_MAXIMA[owner],
    });
    const before = capacity.snapshot();
    rejects(
      () => reserveOperation(capacity),
      `S2 production ${owner} N+1`,
    );
    requireCondition(
      canonicalJson(capacity.snapshot()) ===
        canonicalJson(before),
      "host.self_test",
      `S2 production ${owner} refusal mutated capacity`,
    );
  };
  for (const owner of [
    "fifoBatches",
    "fifoInodes",
    "fifoPathBytes",
  ]) {
    unchangedAfterRefusal(owner, (capacity) =>
      reserveFifoBatch(capacity, 2, 146)
    );
  }
  for (const owner of ["nodeLegs", "sockets"]) {
    unchangedAfterRefusal(owner, (capacity) =>
      reserveNodeLeg(capacity, true)
    );
  }
  for (const owner of ["rubyLegs", "sockets"]) {
    unchangedAfterRefusal(owner, (capacity) =>
      reserveRubyLeg(capacity, true)
    );
  }
  for (const owner of [
    "captureAttempts",
    "psCaptures",
  ]) {
    unchangedAfterRefusal(owner, reserveS2CaptureAttempt);
  }
  for (const owner of [
    "rubyCustodySupervisors",
    "protocolSocketpairs",
    "protocolEndpoints",
    "startupPipes",
    "startupPipeEndpoints",
    "descriptorSlots",
  ]) {
    unchangedAfterRefusal(owner, reserveS2Protocol);
  }
  requireProtocolPeak(S2_DESCRIPTOR_CAPACITY);
  rejects(
    () => requireProtocolPeak(S2_DESCRIPTOR_CAPACITY + 1),
    "S2 descriptor 60",
  );
  return Object.freeze({
    ...ledger,
    productionRefusals: 15,
    descriptorN: 59,
    descriptorRefusal: 60,
  });
}

async function runS2PureSelfTests() {
  const reviewUnion = runS2ReviewUnionStructuralSelfTests();
  const captureCapacity =
    runS2CaptureCapacitySelfTests();
  const productionFaults =
    await runS2ProductionFaultSelfTests();
  const source = runS2SourceStructuralSelfTests();
  const receiptByteOwner = runReceiptByteOwnerSelfTests();
  const receiptTransport = runReceiptTransportSelfTests();
  const fstatDeadline = runFstatDeadlineSelfTests();
  const workerContract = runWorkerContractSelfTests();
  const custody = runCustodySelfTests();
  const capacity = runS2CapacitySelfTests();
  const protocol = await runS2ProtocolSelfTests();
  const faultProtocol = runS2FaultProtocolSelfTests();
  const psParser = runPsParserSelfTests();
  const rubyTopology = runRubyTopologySelfTests();
  const nodeSupport = nodeOutput(
    Buffer.from(
      "SUCCESS|123|122|16777231|9007199254740993\n",
    ),
    true,
  );
  const nodeDenial = nodeOutput(
    Buffer.from("DENIED|123|122|EPERM\n"),
    false,
  );
  const highBit = Buffer.from(
    "SUCCESS|123|122|16777231|9007199254740993\n",
  );
  highBit[0] |= 0x80;
  rejects(
    () => nodeOutput(highBit, true),
    "S2 Node high-bit stdout",
  );
  requireCondition(
    S2_PACKET.version ===
        "VSQ01S2_PHASE_A0_MAIN_V1" &&
      S2_PACKET.scheduler ===
        "5099e31902395ea10d1cca2ee061fc8f3904c748" &&
      S2_PACKET.laneSha256 ===
        "74444d6688e77a75cbb3c0bf52ba4fb854e15b220554cacd2062191ae1970487" &&
      FIXED_PINS.length === 16 &&
      Object.values(PARTITION_CAPS).reduce(
        (total, bytes) => total + bytes,
        0,
      ) === EVIDENCE_MAX_BYTES &&
      Buffer.byteLength(S2_TARGET_LITERAL) < 8_192 &&
      CUSTODY_SUPERVISOR_LITERAL.length > 1_000,
    "host.self_test",
    "S2 packet/static/evidence arithmetic differs",
  );
  return Object.freeze({
    outcome: "SOURCE_ONLY_NON_ACCEPTING",
    packet: S2_PACKET,
    reviewUnion,
    captureCapacity,
    productionFaults,
    source,
    receiptByteOwner,
    receiptTransport,
    fstatDeadline,
    workerContract,
    custody,
    capacity,
    protocol,
    faultProtocol,
    psParser,
    rubyTopology,
    node: Object.freeze({
      support: nodeSupport.result,
      denial: nodeDenial.result,
      highBitRejected: true,
    }),
    targetLiteralSha256: sha256(
      Buffer.from(S2_TARGET_LITERAL),
    ),
    supervisorLiteralSha256: sha256(
      Buffer.from(CUSTODY_SUPERVISOR_LITERAL),
    ),
  });
}

async function runS2FaultKat(outerDeadline) {
  requireCondition(
    process.execPath === NODE &&
      process.argv0 === NODE &&
      process.argv.length === 3 &&
      process.argv[2] === "--s2-fault-kat" &&
      outerDeadline instanceof AbsoluteDeadline,
    "host.s2_fault_kat",
    "S2 fault-KAT invocation differs",
  );
  const facts = await runS2PureSelfTests();
  const productionFaults =
    await runS2ProductionFaultSelfTests();
  const captureCapacity =
    runS2CaptureCapacitySelfTests();
  const nativeFaults =
    await runS2NativeFaultSelfTests(outerDeadline);
  const recurrences = Object.freeze({
    capture: Object.freeze({
      activeProductionSeams: Object.freeze(
        nativeFaults.activeCapture.map((entry) => entry.seam),
      ),
      proofInstalledEvidenceRetained:
        productionFaults.proofInstalledEvidence.retained,
    }),
    capacity: Object.freeze({
      captureRefusal:
        productionFaults.captureNoObject.capacityNPlusOne === 0,
      descriptorRefusal:
        nativeFaults.descriptorBoundary.refusal === 60,
      fifoRefusal:
        nativeFaults.fifoCapacity.refusalBeforeCreate,
    }),
    relayWorker: Object.freeze({
      firstFault:
        productionFaults.nodeStopFirstFault.firstFault,
      secondaryFault:
        productionFaults.nodeStopFirstFault.secondaryFault,
    }),
    transport: Object.freeze({
      faults: facts.protocol.transportFaults,
      noRetry: facts.protocol.transportNoRetry,
    }),
  });
  requireCondition(
    exactPrimitiveArray(
      recurrences.capture.activeProductionSeams,
      ["launch", "output"],
    ) &&
      recurrences.capture.proofInstalledEvidenceRetained === true &&
      recurrences.capacity.captureRefusal === true &&
      recurrences.capacity.descriptorRefusal === true &&
      recurrences.capacity.fifoRefusal === true &&
      recurrences.relayWorker.firstFault === "DEADLINE_STOP" &&
      recurrences.relayWorker.secondaryFault ===
        "host.kat_node_stop_write" &&
      recurrences.transport.faults === 5 &&
      recurrences.transport.noRetry === 3 &&
      facts.faultProtocol.stopNegatives === 6 &&
      facts.capacity.descriptorN === 59 &&
      facts.capacity.descriptorRefusal === 60 &&
      productionFaults.descriptorPhases === 13 &&
      captureCapacity.ordinaryAdmissions === 36 &&
      captureCapacity.replacementAdmissions === 1,
    "host.s2_fault_kat",
    "S2 owner recurrence coverage differs",
    { recurrences, facts },
  );
  const owner = streamOwner(
    outerDeadline.sub("owner", 10_000),
  );
  emitResult({
    code: "host.phase_a0_s2_fault_kat",
    outcome: "SOURCE_ONLY_NON_ACCEPTING",
    sourceSha256: owner.sha256,
    sourceBytes: owner.size,
    recurrences,
    productionFaults,
    captureCapacity,
    nativeFaults,
  }, outerDeadline);
}

async function runSelfTest(outerDeadline) {
  requireCondition(
    process.execPath === NODE &&
      process.argv0 === NODE &&
      process.argv.length === 3 &&
      process.argv[2] === "--self-test" &&
      outerDeadline instanceof AbsoluteDeadline,
    "host.self_test",
    "self-test invocation differs",
  );
  const facts = await runS2PureSelfTests();
  const owner = streamOwner(
    outerDeadline.sub("owner", 10_000),
  );
  emitResult({
    code: "host.phase_a0_self_test",
    outcome: "SOURCE_ONLY_NON_ACCEPTING",
    sourceSha256: owner.sha256,
    sourceBytes: owner.size,
    facts,
  }, outerDeadline);
}

async function main() {
  let resultDeadline;
  try {
    if (process.argv[2] === "--self-test") {
      resultDeadline = AbsoluteDeadline.fromNow(
        "self-test",
        10_000,
      );
      await runSelfTest(resultDeadline);
      return;
    }
    if (process.argv[2] === "--s2-fault-kat") {
      resultDeadline = AbsoluteDeadline.fromNow(
        "s2-fault-kat",
        10_000,
      );
      await runS2FaultKat(resultDeadline);
      return;
    }
    if (process.argv[2] === "--vsq-s2-worker") {
      await runS2Worker(process.argv.slice(3));
      return;
    }
    requireCondition(
      process.argv[2] !== "--vsq-a0-worker",
      "host.legacy_worker_forbidden",
      "legacy S1 worker mode is disabled",
    );
    resultDeadline = AbsoluteDeadline.fromNow(
      "s2-supervisor",
      660_000,
    );
    const exitCode = await runS2Supervisor(resultDeadline);
    process.exitCode = exitCode;
  } catch (error) {
    process.exitCode = 1;
    if (process.argv[2] !== "--vsq-s2-worker") {
      try {
        requireCondition(
          resultDeadline instanceof AbsoluteDeadline,
          "host.result_deadline",
          "STOP result lacks its inherited outer deadline",
        );
        emitResult({
          code:
            typeof error?.code === "string"
              ? error.code
              : "host.internal",
          outcome: "STOP",
          error: safeError(error),
          stateRetained: true,
        }, resultDeadline);
      } catch {
        const fallback = Buffer.from(
          '{"code":"host.result_failure","outcome":"STOP","stateRetained":true}\n',
        );
        if (resultDeadline instanceof AbsoluteDeadline) {
          try {
            writeAll(2, fallback, resultDeadline);
          } catch {
            // An expired outer deadline forbids a new result write.
          }
        }
      }
    }
  }
}

await main();
