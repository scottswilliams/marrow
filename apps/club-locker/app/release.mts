// The toolchain release this app was built against. The trusted main refuses a
// deployment whose manifest names any other release, exactly as the terminal
// companion path refuses a release-mismatched runner. Kept as a standalone pin so a
// deployment directory swapped for a different release is caught, not run.
export const EXPECTED_RELEASE = "0.1.0";
