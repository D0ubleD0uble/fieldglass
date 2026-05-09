import * as vscode from "vscode";
import { FieldglassEditorProvider } from "./provider";

export interface FieldglassApi {
  provider: FieldglassEditorProvider;
}

export function activate(context: vscode.ExtensionContext): FieldglassApi {
  const { provider, disposables } = FieldglassEditorProvider.register(context);
  context.subscriptions.push(...disposables);
  return { provider };
}

export function deactivate(): void {}
