import * as vscode from "vscode";
import { FieldglassEditorProvider } from "./provider";

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(FieldglassEditorProvider.register(context));
}

export function deactivate(): void {}
