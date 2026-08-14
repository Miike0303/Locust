/**
 * Lightweight asserts for ws (run: npx --yes tsx src/lib/ws.test.ts).
 *
 * ws.ts is not pure: it constructs a WebSocket after getWsUrl(). Stub both
 * before the dynamic import. Real lib/api.ts cannot load under tsx
 * (import.meta.env.DEV is undefined), so getWsUrl is stubbed via registerHooks.
 */
import assert from "node:assert/strict";
import { registerHooks } from "node:module";

globalThis.window ??= {} as any;

class FakeWebSocket {
	static instances: FakeWebSocket[] = [];
	url: string;
	closed = false;
	onmessage: ((event: { data: string }) => void) | null = null;
	onerror: ((err?: unknown) => void) | null = null;
	onclose: (() => void) | null = null;

	constructor(url: string) {
		this.url = url;
		FakeWebSocket.instances.push(this);
	}

	close() {
		if (this.closed) return;
		this.closed = true;
		this.onclose?.();
	}

	emitMessage(data: unknown) {
		this.onmessage?.({ data: JSON.stringify(data) });
	}

	emitError(err?: unknown) {
		this.onerror?.(err);
	}

	emitClose() {
		this.onclose?.();
	}
}

(globalThis as any).WebSocket = FakeWebSocket;

(globalThis as any).__locustGetWsUrl = async (
	jobId: string,
	channel = "translate",
) => `ws://localhost:7842/api/${channel}/ws/${jobId}`;

registerHooks({
	load(url, context, nextLoad) {
		const normalized = url.replace(/\\/g, "/");
		if (normalized.endsWith("/src/lib/api.ts")) {
			return {
				format: "module",
				source: `export async function getWsUrl(jobId, channel) {
          return globalThis.__locustGetWsUrl(jobId, channel ?? "translate");
        }`,
				shortCircuit: true,
			};
		}
		return nextLoad(url, context);
	},
});

const {
	JOB_STREAM_LOST_MESSAGE,
	subscribeToJob,
	waitForJob,
} = await import("./ws.ts");

assert.equal(JOB_STREAM_LOST_MESSAGE, "ws.jobStreamLost");

function reset() {
	FakeWebSocket.instances.length = 0;
	(globalThis as any).__locustGetWsUrl = async (
		jobId: string,
		channel = "translate",
	) => `ws://localhost:7842/api/${channel}/ws/${jobId}`;
}

function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (reason?: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

function tick() {
	return new Promise((r) => setTimeout(r, 0));
}

async function lastSocket(): Promise<FakeWebSocket> {
	await tick();
	const ws = FakeWebSocket.instances.at(-1);
	if (!ws) throw new Error("expected a WebSocket to be constructed");
	return ws;
}

// 1. Unsubscribing before the socket opens still closes it and suppresses handlers.
{
	reset();
	const pending = deferred<string>();
	(globalThis as any).__locustGetWsUrl = () => pending.promise;

	let started = false;
	let closed = false;
	const unsub = subscribeToJob("job-early-unsub", {
		onStarted: () => {
			started = true;
		},
		onClosed: () => {
			closed = true;
		},
	});
	unsub();
	pending.resolve("ws://localhost:7842/api/translate/ws/job-early-unsub");
	await pending.promise;
	await tick();

	assert.equal(FakeWebSocket.instances.length, 1);
	const socket = FakeWebSocket.instances[0];
	assert.equal(socket.closed, true);
	socket.emitMessage({ type: "started", total: 1, job_id: "job-early-unsub" });
	socket.emitClose();
	assert.equal(started, false);
	assert.equal(closed, false);
}

// 2. A close with no terminal event rejects waitForJob.
{
	reset();
	const job = waitForJob("job-drop");
	const socket = await lastSocket();
	socket.emitClose();
	await assert.rejects(job, (err: unknown) => {
		assert.ok(err instanceof Error);
		assert.equal(err.message, JOB_STREAM_LOST_MESSAGE);
		return true;
	});
}

// Socket error takes the same terminal path.
{
	reset();
	const origError = console.error;
	console.error = () => {};
	try {
		const job = waitForJob("job-error");
		const socket = await lastSocket();
		socket.emitError(new Error("boom"));
		await assert.rejects(job, (err: unknown) => {
			assert.ok(err instanceof Error);
			assert.equal(err.message, JOB_STREAM_LOST_MESSAGE);
			return true;
		});
	} finally {
		console.error = origError;
	}
}

// 3. A close arriving after completed leaves the resolved promise alone.
{
	reset();
	const job = waitForJob("job-complete");
	const socket = await lastSocket();
	socket.emitMessage({
		type: "completed",
		total_translated: 1,
		total_cost: 0,
		duration_secs: 0,
	});
	socket.emitClose();
	await job;
}

// getWsUrl rejection takes the same terminal path (hang fix).
{
	reset();
	const origError = console.error;
	console.error = () => {};
	try {
		(globalThis as any).__locustGetWsUrl = async () => {
			throw new Error("port lookup failed");
		};
		await assert.rejects(waitForJob("job-url-fail"), (err: unknown) => {
			assert.ok(err instanceof Error);
			assert.equal(err.message, JOB_STREAM_LOST_MESSAGE);
			return true;
		});
	} finally {
		console.error = origError;
	}
}

// Patch-apply channel: progress/done/error frames, and a drop without a
// terminal frame still fires onClosed (same hang-fix as translation).
{
	reset();
	const urls: string[] = [];
	(globalThis as any).__locustGetWsUrl = async (jobId: string, channel?: string) => {
		const url = `ws://localhost:7842/api/${channel ?? "translate"}/ws/${jobId}`;
		urls.push(url);
		return url;
	};

	const progress: unknown[] = [];
	let done: unknown = null;
	let closed = false;
	subscribeToJob(
		"patch-job",
		{
			onProgress: (e) => progress.push(e),
			onDone: (e) => {
				done = e.report;
			},
			onClosed: () => {
				closed = true;
			},
		},
		"patch",
	);
	const socket = await lastSocket();
	assert.equal(urls.at(-1), "ws://localhost:7842/api/patch/ws/patch-job");
	socket.emitMessage({
		type: "progress",
		current: 3,
		total: 10,
		path: "Foo.pak",
		phase: "write",
	});
	assert.equal(progress.length, 1);
	socket.emitMessage({
		type: "done",
		report: { patch_id: "p1", patch_version: "1", replaced: 3, added: 0 },
	});
	assert.equal((done as { patch_id: string }).patch_id, "p1");
	socket.emitClose();
	assert.equal(closed, true);
}

{
	reset();
	let closed = false;
	let errored = false;
	subscribeToJob(
		"patch-drop",
		{
			onError: () => {
				errored = true;
			},
			onClosed: () => {
				closed = true;
			},
		},
		"patch",
	);
	const socket = await lastSocket();
	socket.emitClose();
	assert.equal(errored, false);
	assert.equal(closed, true);
}

{
	reset();
	let message = "";
	subscribeToJob(
		"patch-err",
		{
			onError: (e) => {
				message = e.message;
			},
		},
		"patch",
	);
	const socket = await lastSocket();
	socket.emitMessage({ type: "error", message: "disk full" });
	assert.equal(message, "disk full");
}

console.log("ws.test.ts: ok");
