import {
  catchError,
  defaultIfEmpty,
  defer,
  exhaustMap,
  map,
  Observable,
  of,
  scan,
  startWith,
  timeout,
} from 'rxjs';

export interface VerificationPoll<T> {
  data: T | null;
  loading: boolean;
  error: boolean;
  unsupported: boolean;
  updatedAt: number | null;
}

/** Only explicit capability failures mean unsupported; an empty/slow RPC is busy. */
export function verificationUnsupported(error: any): boolean {
  return (
    error?.name === 'VerificationUnsupportedError' ||
    [404, 405, 501].includes(error?.status) ||
    /Cannot query field ["']verificationReceipts["']/.test(error?.message ?? '')
  );
}

/** Scoped to one node/query. A failed poll never erases successful evidence. */
export function verificationPoll<T>(
  triggers: Observable<unknown>,
  request: () => Observable<T>,
  timeoutMs: number,
  accept: (data: T, previous: T | null) => boolean = () => true,
): Observable<VerificationPoll<T>> {
  const initial: VerificationPoll<T> = {
    data: null,
    loading: true,
    error: false,
    unsupported: false,
    updatedAt: null,
  };
  return triggers.pipe(
    exhaustMap(() =>
      defer(request).pipe(
        timeout(timeoutMs),
        map(data => ({ data, error: false, unsupported: false })),
        defaultIfEmpty({ data: null, error: true, unsupported: false }),
        catchError(error =>
          of({
            data: null,
            error: true,
            unsupported: verificationUnsupported(error),
          }),
        ),
      ),
    ),
    scan((previous, answer): VerificationPoll<T> => {
      const success =
        !answer.error &&
        answer.data != null &&
        accept(answer.data, previous.data);
      return {
        data: success ? answer.data : previous.data,
        updatedAt: success ? Date.now() : previous.updatedAt,
        loading: false,
        error: !success,
        unsupported: answer.unsupported,
      };
    }, initial),
    startWith(initial),
  );
}

export function updatedSeconds(
  updatedAt: number | null,
  now: number,
): number | null {
  return updatedAt === null
    ? null
    : Math.max(0, Math.floor((now - updatedAt) / 1000));
}
