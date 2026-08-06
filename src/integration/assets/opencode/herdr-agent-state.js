// installed by herdr
// managed by herdr; reinstalling or updating the integration overwrites this file.
// add custom hooks/plugins beside this file instead of editing it.
// HERDR_INTEGRATION_ID=opencode
// HERDR_INTEGRATION_VERSION=10

// Dual-API integration:
// - opencode v2 loads the default export ({ id, setup } using ctx.event.subscribe).
// - opencode v1 loads the HerdrAgentStatePlugin named export (chat.message/event hooks).
// Both runtimes ignore the export shape they do not understand, so one file
// serves opencode 1.x and the opencode2 beta from the shared plugins directory.
//
// opencode2 runs one shared background service per user: its plugins live in
// the service process and inherit the pane env of whichever pane started the
// service. Reports therefore carry a session directory, which herdr matches
// against the panes' cwd to attribute each session to the right pane,
// falling back to the reported pane id when no directory is available.

import net from "node:net";

const SOURCE = "herdr:opencode";
const AGENT = "opencode";
let reportSeq = Date.now() * 1000;
let requestChain = Promise.resolve();
let reportedRootSessionID;

// Subagent (task tool) sessions carry a parentID; the main agent session does
// not. Their lifecycle events would otherwise clobber the pane's real state, so
// learn child session ids from session.created and drop their reports.
const childSessions = new Set();

// Permission and question prompts are user-facing regardless of which session
// asked them, and they must never attach that session's id: a subagent session
// the plugin never saw created (a service restart drops session.created from
// the replay window) would otherwise replace the pane's root session or be
// dropped by herdr as cross-talk, leaving the pane frozen in a stale state.
// Project them state-only so the pane keeps its root session and the
// blocked/working flip always lands on the pane that hosts the session path.
const CHILD_EVENT_STATES = new Map([
  ["permission.asked", "blocked"],
  ["question.asked", "blocked"],
  ["permission.replied", "working"],
  ["question.replied", "working"],
  ["question.rejected", "working"],
]);

const WORKING_EVENT_TYPES = new Set([
  "session.execution.started",
  "session.input.admitted",
  "session.input.promoted",
  "session.synthetic",
  "session.step.started",
  "session.shell.started",
  "session.reasoning.started",
  "session.text.started",
  "session.tool.input.started",
  "session.tool.called",
  "session.tool.progress",
  "session.compaction.started",
  "session.compacted",
  "session.retry.scheduled",
  "permission.replied",
  "question.replied",
  "question.rejected",
]);

const BLOCKED_EVENT_TYPES = new Set([
  "permission.asked",
  "question.asked",
  "session.execution.failed",
]);

// opencode2 emits no session.idle event. A turn ends with session.step.ended
// carrying finish "stop" (no tool call follows), followed by
// session.execution.succeeded; both mean the session is waiting for the user
// again, so they project idle instead of leaving the pane working forever.
// session.execution.* events carry no location, so every event's directory is
// remembered per session for routing those final reports.
const sessionPaths = new Map();
const lastReportedStateBySession = new Map();

function sessionPathFor(sessionID) {
  if (!sessionID) return undefined;
  return sessionPaths.get(sessionID);
}

function shouldSuppressRepeatedState(state, sessionID) {
  if (!sessionID) {
    return false;
  }
  if (lastReportedStateBySession.get(sessionID) === state) {
    return true;
  }
  lastReportedStateBySession.set(sessionID, state);
  return false;
}

function nextReportSeq() {
  reportSeq += 1;
  return reportSeq;
}

// opencode v2 durable events carry the session id in data.sessionID or in
// durable.aggregateID; opencode v1 events carry it in properties.sessionID.
function sessionIDFromEvent(event) {
  const data = event?.data ?? {};
  const durable = event?.durable ?? {};
  return (
    (typeof data.sessionID === "string" && data.sessionID) ||
    (typeof durable.aggregateID === "string" && durable.aggregateID) ||
    (typeof event?.properties?.sessionID === "string" &&
      event.properties.sessionID) ||
    undefined
  );
}

// opencode v2 durable events carry the session's project directory in
// location.directory (data.location.directory on session.created); opencode v1
// events carry no location, so the path stays undefined there.
function sessionPathFromEvent(event) {
  const direct = event?.location?.directory;
  const nested = event?.data?.location?.directory;
  if (typeof direct === "string" && direct.length > 0) {
    return direct;
  }
  if (typeof nested === "string" && nested.length > 0) {
    return nested;
  }
  return undefined;
}

// session.status carries { type: "idle" | "busy" | "retry" }; older builds used a bare string.
function stateFromSessionStatus(status) {
  const kind = typeof status === "string" ? status : status?.type;
  if (typeof kind !== "string") return undefined;
  switch (kind.toLowerCase()) {
    case "idle":
      return "idle";
    case "active":
    case "busy":
    case "pending":
    case "running":
    case "streaming":
    case "working":
    case "retry":
      return "working";
    default:
      return undefined;
  }
}

function request(method, params) {
  const pending = requestChain.then(() => requestOnce(method, params));
  requestChain = pending.catch(() => {});
  return pending;
}

function requestOnce(method, params) {
  const paneId = process.env.HERDR_PANE_ID;
  const socketPath = process.env.HERDR_SOCKET_PATH;

  if (!paneId || !socketPath) {
    return Promise.resolve();
  }

  const socketEndpoint =
    process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;

  const requestId = `${SOURCE}:${Date.now()}:${Math.floor(Math.random() * 1_000_000)
    .toString()
    .padStart(6, "0")}`;
  const request = {
    id: requestId,
    method,
    params: {
      pane_id: paneId,
      source: SOURCE,
      agent: AGENT,
      seq: nextReportSeq(),
      ...params,
    },
  };

  return new Promise((resolve) => {
    const client = net.createConnection(socketEndpoint, () => {
      client.write(`${JSON.stringify(request)}\n`);
    });

    const finish = () => {
      client.destroy();
      resolve();
    };

    client.setTimeout(500, finish);
    client.on("data", finish);
    client.on("error", finish);
    client.on("end", finish);
    client.on("close", resolve);
  });
}
function reportSession(sessionID, sessionStartSource, sessionPath) {
  if (!sessionID) {
    return Promise.resolve();
  }
  const params = { agent_session_id: sessionID };
  if (sessionStartSource) {
    params.session_start_source = sessionStartSource;
  }
  const path = sessionPath ?? sessionPathFor(sessionID);
  if (path) {
    params.agent_session_path = path;
  }
  return request("pane.report_agent_session", params);
}

function reportState(state, sessionID, sessionPath) {
  if (shouldSuppressRepeatedState(state, sessionID)) {
    return Promise.resolve();
  }
  const params = { state };
  if (sessionID) {
    reportedRootSessionID = sessionID;
    params.agent_session_id = sessionID;
  }
  const path = sessionPath ?? sessionPathFor(sessionID);
  if (path) {
    params.agent_session_path = path;
  }
  return request("pane.report_agent", params);
}

function handleEvent(event) {
  const type = event?.type;
  const sessionID = sessionIDFromEvent(event);
  const sessionPath = sessionPathFromEvent(event);

  if (sessionID && sessionPath) {
    sessionPaths.set(sessionID, sessionPath);
  }

  // opencode v2 session.created data carries the parent session id for
  // subagent sessions; opencode v1 carries it in properties.info.parentID.
  const parentID =
    (typeof event?.data?.parentID === "string" && event.data.parentID) ||
    (typeof event?.properties?.info?.parentID === "string" &&
      event.properties.info.parentID);
  if (sessionID && parentID) {
    childSessions.add(sessionID);
  }
  if (sessionID && CHILD_EVENT_STATES.has(type)) {
    // Project state only, without an agent_session_id, so the pane keeps the
    // root session. This covers root sessions and any subagent session, known
    // or not.
    return reportState(CHILD_EVENT_STATES.get(type), undefined, sessionPath);
  }
  if (sessionID && childSessions.has(sessionID)) {
    return Promise.resolve();
  }

  switch (type) {
    case "session.created":
      // A root session.created is a genuine new-session start (subagent
      // creates are dropped above). Signal it so herdr replaces the pane's
      // prior session id instead of treating the change as cross-talk.
      return reportSession(sessionID, "new", sessionPath);
    case "session.updated":
      if (sessionID && sessionID !== reportedRootSessionID) {
        return reportSession(sessionID, undefined, sessionPath);
      }
      return Promise.resolve();
    case "session.status": {
      const state = stateFromSessionStatus(
        event?.data?.status ?? event?.properties?.status,
      );
      if (state) {
        return reportState(state, sessionID, sessionPath);
      }
      return reportSession(sessionID, undefined, sessionPath);
    }
    case "session.step.ended": {
      // finish "stop" ends the turn without tool calls, so the session is
      // waiting for the user again; other finishes continue working.
      const finish = event?.data?.finish ?? event?.properties?.finish;
      if (finish === "stop") {
        return reportState("idle", sessionID, sessionPath);
      }
      return reportState("working", sessionID, sessionPath);
    }
    case "session.execution.succeeded":
      return reportState("idle", sessionID, sessionPath);
    case "session.idle":
      return reportState("idle", sessionID, sessionPath);
    case "session.deleted":
      return Promise.resolve();
    default:
      if (WORKING_EVENT_TYPES.has(type)) {
        return reportState("working", sessionID, sessionPath);
      }
      if (BLOCKED_EVENT_TYPES.has(type)) {
        return reportState("blocked", sessionID, sessionPath);
      }
      return Promise.resolve();
  }
}

export const HerdrAgentStatePlugin = async () => {
  if (
    process.env.HERDR_ENV !== "1" ||
    !process.env.HERDR_SOCKET_PATH ||
    !process.env.HERDR_PANE_ID
  ) {
    return {};
  }

  return {
    "chat.message": async ({ sessionID }) => {
      if (sessionID && childSessions.has(sessionID)) {
        return;
      }
      await reportState("working", sessionID);
    },
    event: async ({ event }) => {
      await handleEvent(event);
    },
  };
};

export default {
  id: "herdr.agent-state",
  setup: async (ctx) => {
    if (
      process.env.HERDR_ENV !== "1" ||
      !process.env.HERDR_SOCKET_PATH ||
      !process.env.HERDR_PANE_ID
    ) {
      return;
    }

    const events = ctx.event.subscribe();
    const pump = async () => {
      for await (const event of events) {
        try {
          await handleEvent(event);
        } catch {
          // State reporting must never break the session it observes.
        }
      }
    };
    pump();
  },
};
