import { beforeEach, expect, mock, test } from "bun:test";

const requests: unknown[] = [];
const clients: FakeClient[] = [];
const requestWaiters: Array<() => void> = [];
let autoAcknowledge = true;
let importCounter = 0;

type FakeClient = {
  emit: (event: string) => void;
};

mock.module("node:net", () => ({
  default: {
    createConnection(_path: string, onConnect: () => void) {
      const handlers = new Map<string, () => void>();
      const client = {
        write(input: string) {
          requests.push(JSON.parse(input.trim()));
          requestWaiters.shift()?.();
          if (autoAcknowledge) {
            queueMicrotask(() => client.emit("data"));
          }
        },
        setTimeout() {},
        on(event: string, handler: () => void) {
          handlers.set(event, handler);
        },
        destroy() {},
        emit(event: string) {
          handlers.get(event)?.();
        },
      };
      clients.push(client);
      queueMicrotask(onConnect);
      return client;
    },
  },
}));

beforeEach(() => {
  requests.length = 0;
  clients.length = 0;
  requestWaiters.length = 0;
  autoAcknowledge = true;
  process.env.HERDR_ENV = "1";
  process.env.HERDR_SOCKET_PATH = "test.sock";
  process.env.HERDR_PANE_ID = "test:p1";
});

async function loadPlugin() {
  importCounter += 1;
  const { HerdrAgentStatePlugin } = await import(`./herdr-agent-state.js?test=${importCounter}`);
  return HerdrAgentStatePlugin();
}

function waitForNextRequest(): Promise<void> {
  return new Promise((resolve) => requestWaiters.push(resolve));
}

test("serializes lifecycle reports", async () => {
  autoAcknowledge = false;
  const plugin = await loadPlugin();
  const firstDispatched = waitForNextRequest();
  const working = plugin.event({
    event: {
      type: "session.status",
      properties: { sessionID: "root-session", status: { type: "busy" } },
    },
  });
  await firstDispatched;

  const secondDispatched = waitForNextRequest();
  const idle = plugin.event({
    event: {
      type: "session.status",
      properties: { sessionID: "root-session", status: { type: "idle" } },
    },
  });
  expect(clients).toHaveLength(1);

  clients[0]?.emit("data");
  await secondDispatched;
  expect(clients).toHaveLength(2);
  clients[1]?.emit("data");
  await Promise.all([working, idle]);

  expect(requests.map(requestState)).toEqual(["working", "idle"]);
  const sequences = requests.map(requestSeq);
  expect(sequences[0]).toEqual(expect.any(Number));
  expect(sequences[1]).toBe((sequences[0] as number) + 1);
});

test("suppresses redundant same-session updates", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.status",
      properties: { sessionID: "root-session", status: { type: "busy" } },
    },
  });
  await plugin.event({
    event: { type: "session.updated", properties: { sessionID: "root-session" } },
  });
  await plugin.event({
    event: { type: "session.updated", properties: { sessionID: "replacement-session" } },
  });

  expect(requests.map(requestMethod)).toEqual([
    "pane.report_agent",
    "pane.report_agent_session",
  ]);
  expect(requests.map(requestSessionID)).toEqual(["root-session", "replacement-session"]);
});

test("reports retry status as working", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.status",
      properties: { sessionID: "root-session", status: { type: "retry" } },
    },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestState)).toEqual(["working"]);
  expect(requests.map(requestSessionID)).toEqual(["root-session"]);
});

test("reports child prompts without replacing the root session", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.created",
      properties: {
        sessionID: "child-session",
        info: { id: "child-session", parentID: "root-session" },
      },
    },
  });

  for (const type of ["permission.asked", "question.asked"]) {
    await plugin.event({ event: { type, properties: { sessionID: "child-session" } } });
  }
  for (const type of ["permission.replied", "question.replied", "question.rejected"]) {
    await plugin.event({ event: { type, properties: { sessionID: "child-session" } } });
  }

  expect(requests.map(requestState)).toEqual([
    "blocked",
    "blocked",
    "working",
    "working",
    "working",
  ]);
  expect(requests.map(requestSessionID)).toEqual([
    undefined,
    undefined,
    undefined,
    undefined,
    undefined,
  ]);
});

test("reports the session directory with new session reports", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.created",
      data: { sessionID: "root-session", location: { directory: "/work/proj" } },
    },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent_session"]);
  expect(requests.map(requestSessionID)).toEqual(["root-session"]);
  expect(requests.map(requestSessionPath)).toEqual(["/work/proj"]);
});
test("reports the session directory with state reports", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.status",
      data: { sessionID: "root-session", status: { type: "busy" } },
      location: { directory: "/work/proj" },
    },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestState)).toEqual(["working"]);
  expect(requests.map(requestSessionID)).toEqual(["root-session"]);
  expect(requests.map(requestSessionPath)).toEqual(["/work/proj"]);
});
test("reports the session directory with child session state projections", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.created",
      data: {
        sessionID: "child-session",
        parentID: "root-session",
        location: { directory: "/work/proj" },
      },
    },
  });
  await plugin.event({
    event: {
      type: "permission.asked",
      data: { sessionID: "child-session" },
      location: { directory: "/work/proj" },
    },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestState)).toEqual(["blocked"]);
  expect(requests.map(requestSessionID)).toEqual([undefined]);
  expect(requests.map(requestSessionPath)).toEqual(["/work/proj"]);
});
test("omits the session path when events carry no location", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.created",
      data: { sessionID: "root-session" },
    },
  });
  await plugin.event({
    event: {
      type: "session.status",
      data: { sessionID: "root-session", status: { type: "busy" } },
    },
  });
  await plugin.event({
    event: { type: "session.idle", data: { sessionID: "root-session" } },
  });

  expect(requests.map(requestMethod)).toEqual([
    "pane.report_agent_session",
    "pane.report_agent",
    "pane.report_agent",
  ]);
  expect(requests.map(requestSessionPath)).toEqual([undefined, undefined, undefined]);
});

test("reports a stop-finished step as idle", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.step.ended",
      data: { sessionID: "root-session", finish: "stop" },
      location: { directory: "/work/proj" },
    },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestState)).toEqual(["idle"]);
  expect(requests.map(requestSessionID)).toEqual(["root-session"]);
  expect(requests.map(requestSessionPath)).toEqual(["/work/proj"]);
});

test("reports a tool-call step finish as working", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.step.ended",
      data: { sessionID: "root-session", finish: "tool-calls" },
      location: { directory: "/work/proj" },
    },
  });

  expect(requests.map(requestMethod)).toEqual(["pane.report_agent"]);
  expect(requests.map(requestState)).toEqual(["working"]);
});

test("reports execution success as idle using the cached session path", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.step.started",
      data: { sessionID: "root-session" },
      location: { directory: "/work/proj" },
    },
  });
  await plugin.event({
    event: {
      type: "session.execution.succeeded",
      data: { sessionID: "root-session" },
    },
  });

  expect(requests.map(requestMethod)).toEqual([
    "pane.report_agent",
    "pane.report_agent",
  ]);
  expect(requests.map(requestState)).toEqual(["working", "idle"]);
  expect(requests.map(requestSessionPath)).toEqual(["/work/proj", "/work/proj"]);
});

test("projects permission states from sessions never seen created", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.created",
      data: { sessionID: "root-session", location: { directory: "/work/proj" } },
    },
  });
  await plugin.event({
    event: {
      type: "permission.asked",
      data: { sessionID: "unknown-subagent" },
      location: { directory: "/work/proj" },
    },
  });
  await plugin.event({
    event: {
      type: "permission.replied",
      data: { sessionID: "unknown-subagent" },
      location: { directory: "/work/proj" },
    },
  });

  expect(requests.map(requestMethod)).toEqual([
    "pane.report_agent_session",
    "pane.report_agent",
    "pane.report_agent",
  ]);
  expect(requests.map(requestState)).toEqual([undefined, "blocked", "working"]);
  expect(requests.map(requestSessionID)).toEqual([
    "root-session",
    undefined,
    undefined,
  ]);
  expect(requests.map(requestSessionPath)).toEqual(["/work/proj", "/work/proj", "/work/proj"]);
});

test("suppresses repeated same-session state reports", async () => {
  const plugin = await loadPlugin();

  await plugin.event({
    event: {
      type: "session.tool.called",
      data: { sessionID: "root-session" },
      location: { directory: "/work/proj" },
    },
  });
  await plugin.event({
    event: {
      type: "session.step.started",
      data: { sessionID: "root-session" },
      location: { directory: "/work/proj" },
    },
  });
  await plugin.event({
    event: { type: "session.execution.succeeded", data: { sessionID: "root-session" } },
  });

  expect(requests.map(requestState)).toEqual(["working", "idle"]);
});
function requestMethod(request: unknown): unknown {
  return isRecord(request) ? request.method : undefined;
}

function requestState(request: unknown): unknown {
  return requestParam(request, "state");
}

function requestSeq(request: unknown): unknown {
  return requestParam(request, "seq");
}

function requestSessionID(request: unknown): unknown {
  return requestParam(request, "agent_session_id");
}

function requestSessionPath(request: unknown): unknown {
  return requestParam(request, "agent_session_path");
}
function requestParam(request: unknown, name: string): unknown {
  if (!isRecord(request) || !isRecord(request.params)) {
    return undefined;
  }
  return request.params[name];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
