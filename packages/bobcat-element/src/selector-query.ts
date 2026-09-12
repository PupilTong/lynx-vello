export interface NodeSelectToken {
  type: number;
  identifier: string;
  component_id: string;
  first_only: boolean;
  root_unique_id: number | undefined;
}

export type QueryOperation = "fields" | "path" | "setNativeProps" | "invoke";
export interface NodeQueryRequest {
  bobcat: "runtime";
  method: "nodeQuery";
  operation: QueryOperation;
  token: NodeSelectToken;
  params: unknown;
  id?: number;
}

export interface QueryStatus { code: number; data: unknown }
export interface QueryResult<T> { data: T; status: QueryStatus }
export interface QueryNode {
  unique_id?: number;
  query?: SelectorQuery;
  [field: string]: unknown;
}
export type SendQuery = (
  operation: QueryOperation, token: NodeSelectToken, params: unknown,
  callback?: (result: unknown) => void,
) => void;

// Lynx's query builder owns tasks and selection tokens, never MTS handles.
// Native evidence: lynx-core/src/modules/selectorQuery/SelectorQuery.ts.
export class SelectorQuery {
  readonly send: SendQuery;
  readonly report: (error: unknown) => void;
  readonly component: string;
  readonly tasks: (() => void)[];
  immediate: boolean;
  root: number | undefined;

  constructor(send: SendQuery, report: (error: unknown) => void, component = "", tasks: (() => void)[] = []) {
    this.send = send;
    this.report = report;
    this.component = component;
    this.tasks = tasks;
    this.immediate = false;
    this.root = undefined;
  }
  commitTask(task: () => void): SelectorQuery | undefined {
    const next = new SelectorQuery(this.send, this.report, this.component, [...this.tasks, task]);
    if (this.immediate) { next.exec(); return undefined; }
    return next;
  }
  in<T>(component: { createSelectorQuery(query: SelectorQuery): T }): T { return component.createSelectorQuery(this); }
  node(type: number, identifier: string, first_only: boolean) {
    return new NodesRef(this, {type, identifier, first_only,
      component_id: this.component, root_unique_id: this.root});
  }
  select(selector: string) { return this.node(0, selector, true); }
  selectAll(selector: string) { return this.node(0, selector, false); }
  selectRoot() { return this.select(""); }
  selectUniqueID(id: string | number) { return this.node(2, id.toString(), true); }
  selectReactRef(name: string) {
    if (this.tasks.length) {
      this.report(new Error("selectReactRef() should be called before any other selector query methods"));
      return undefined;
    }
    this.immediate = true;
    return this.node(1, name, true);
  }
  setRoot(id: number | string) { this.root = Number(id); return this; }
  exec() { for (const task of this.tasks) task(); }
}

export class NodesRef {
  readonly _selectorQuery: SelectorQuery;
  readonly _nodeSelectToken: NodeSelectToken;

  constructor(query: SelectorQuery, token: NodeSelectToken) {
    this._selectorQuery = query;
    // The compiled portal serializer reads this exact native token shape.
    this._nodeSelectToken = token;
  }
  fields(fields: Record<string, unknown>, callback?: (data: QueryNode | QueryNode[] | null, status: QueryStatus) => void) {
    const query = this._selectorQuery, token = this._nodeSelectToken;
    return query.commitTask(() => {
      const names = [];
      for (const key in fields) {
        if (key === "query" && fields[key] == true && !fields['unique_id']) names.push("unique_id");
        else if (fields[key]) names.push(key);
      }
      query.send("fields", token, names, response => {
        const result = response as QueryResult<QueryNode | QueryNode[] | null>;
        if (fields['query']) {
          const addQuery = (node: QueryNode) => {
            node.query = new SelectorQuery(query.send, query.report).setRoot(node.unique_id!.toString());
            if (!fields['unique_id']) delete node.unique_id;
          };
          if (token.first_only) { if (result.data) addQuery(result.data as QueryNode); }
          else for (const node of result.data as QueryNode[]) addQuery(node);
        }
        callback?.(result.data, result.status);
      });
    });
  }
  path(callback?: (data: QueryNode[] | QueryNode[][] | null, status: QueryStatus) => void) {
    const query = this._selectorQuery;
    return query.commitTask(() => query.send("path", this._nodeSelectToken, null,
      response => {
        const result = response as QueryResult<QueryNode[] | QueryNode[][] | null>;
        callback?.(result.data, result.status);
      }));
  }
  setNativeProps(props: Record<string, unknown>) {
    const query = this._selectorQuery;
    return query.commitTask(() => query.send("setNativeProps", this._nodeSelectToken, props));
  }
  invoke(options: { method: string; params?: unknown; success?: (data: unknown) => void; fail?: (result: QueryStatus) => void }) {
    const query = this._selectorQuery, token = this._nodeSelectToken;
    return query.commitTask(() => {
      const callback = (response: unknown) => {
        const result = response as QueryStatus;
        if (result.code === 0) options.success?.(result.data);
        else if (options.fail) options.fail(result);
        // Native's production facade silently ignores an unhandled failure.
      };
      if (!token.first_only) {
        callback({code: 5, data: "selectAll not supported for invoke method"});
        return;
      }
      query.send("invoke", token, {method: options.method, params: options.params ?? {}}, callback);
    });
  }
}
