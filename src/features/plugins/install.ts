import { open } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../../store/appStore";
import { reasonFromCause } from "../../lib/tauri";

/**
 * Ask where the plugin is, then install it.
 *
 * **There is no download yet, and this does not pretend there is one.** A
 * plugin is a folder with a manifest; nothing publishes those anywhere, so the
 * only source that exists today is a folder on this machine. The states around
 * this call — absent, installing, installed — are the same ones a download will
 * report when there is something to download from, so only this function
 * changes then.
 *
 * Returns false when the person cancelled, which is not a failure and must not
 * be shown as one.
 */
export async function chooseAndInstall(id: string, label: string): Promise<boolean> {
  // Starting a new chooser flow means the previous failure is no longer the
  // current action, even if the person cancels before a folder is selected.
  useAppStore.setState({ installError: null });
  let selected: string | string[] | null;
  try {
    selected = await open({
      directory: true,
      multiple: false,
      title: `Choose the folder ${label} was unpacked into`,
    });
  } catch (cause) {
    useAppStore.setState({ installError: reasonFromCause(cause) });
    return false;
  }
  if (typeof selected !== "string") return false;
  return useAppStore.getState().installPlugin(id, selected);
}
