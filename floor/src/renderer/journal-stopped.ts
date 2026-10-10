// SPDX-License-Identifier: MIT OR Apache-2.0

export interface JournalStopped {
  readonly lsn: string;
}

const U64_MAX = '18446744073709551615';

export function decodeJournalStopped(value: unknown): JournalStopped | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const lsn = (value as { lsn?: unknown }).lsn;
  if (typeof lsn !== 'string' || !/^(0|[1-9][0-9]{0,19})$(?![\s\S])/.test(lsn)
    || (lsn.length === U64_MAX.length && lsn > U64_MAX)) return null;
  return Object.freeze({ lsn });
}
