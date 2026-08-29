import { useCallback, useEffect, useRef, useState } from "react";
import { commandErrorMessage } from "./oracleUtils";

export type RequestState<T> =
  | { status: "loading" }
  | { status: "ready"; value: T }
  | { status: "error"; message: string };

export type TrackedRequestState<T> = { status: "idle" } | RequestState<T>;

export function useTrackedRequest<T>(
  request: () => Promise<T>,
  initialState: TrackedRequestState<T>,
  autoStart = false,
): {
  state: TrackedRequestState<T>;
  run: (showLoadingState?: boolean) => void;
  reset: () => void;
} {
  const [state, setState] = useState<TrackedRequestState<T>>(initialState);
  const initialStateRef = useRef(initialState);
  const mountedRef = useRef(false);
  const requestIdRef = useRef(0);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const executeRequest = useCallback(
    (requestId: number) => {
      void Promise.resolve()
        .then(request)
        .then((value) => {
          if (mountedRef.current && requestIdRef.current === requestId) {
            setState({ status: "ready", value });
          }
        })
        .catch((error: unknown) => {
          if (mountedRef.current && requestIdRef.current === requestId) {
            setState({
              status: "error",
              message: commandErrorMessage(error),
            });
          }
        });
    },
    [request],
  );

  const run = useCallback(
    (showLoadingState = true) => {
      const requestId = requestIdRef.current + 1;
      requestIdRef.current = requestId;
      if (showLoadingState) setState({ status: "loading" });
      executeRequest(requestId);
    },
    [executeRequest],
  );

  const reset = useCallback(() => {
    requestIdRef.current += 1;
    if (mountedRef.current) setState(initialStateRef.current);
  }, []);

  useEffect(() => {
    if (!autoStart) {
      requestIdRef.current += 1;
      setState(initialStateRef.current);
      return;
    }
    const requestId = requestIdRef.current + 1;
    requestIdRef.current = requestId;
    executeRequest(requestId);
  }, [autoStart, executeRequest]);

  return { state, run, reset };
}
