// SPDX-License-Identifier: MIT OR Apache-2.0

const CORE = '(?:0|[1-9][0-9]*)';
const PRERELEASE = '(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)';
const RELEASE = new RegExp(`^${CORE}\\.${CORE}\\.${CORE}(?:-${PRERELEASE}(?:\\.${PRERELEASE})*)?(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?$`);

export function validReleaseVersion(value: unknown): value is string {
  return typeof value === 'string' && value.length <= 128 && RELEASE.test(value);
}
