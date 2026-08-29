// Compatibility re-export while feature modules migrate to the generated
// endpoint client directly.
export {
  requestEndpoint,
  requestJson,
} from "../../generated/api/client";
export type {
  ApiErrorPayload,
  ApiRequestOptions,
} from "../../generated/api/client";
