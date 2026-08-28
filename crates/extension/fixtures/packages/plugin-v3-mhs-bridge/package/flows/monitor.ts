type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

type FlowEventEnvelope = {
  event: {
    type: string;
    step_id?: string;
    output?: JsonValue;
  };
};

type WorkflowInvocation = {
  input: JsonValue;
  history: FlowEventEnvelope[];
};

type StepInvocation = {
  input: JsonValue;
};

export async function run(invocation: WorkflowInvocation) {
  const completed = invocation.history.find(
    (item) => item.event.type === "step_completed" && item.event.step_id === "observe",
  );
  if (completed) {
    return { type: "complete", output: completed.event.output ?? null };
  }
  return {
    type: "schedule_step",
    step_id: "observe",
    step_name: "observe_mhs_fleet",
    input: invocation.input,
    retry: { max_attempts: 1, delay_ms: 0 },
  };
}

export const steps = {
  async observe_mhs_fleet(invocation: StepInvocation) {
    return {
      status: "host-observation-required",
      input: invocation.input,
    };
  },
};
