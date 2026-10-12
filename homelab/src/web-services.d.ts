interface Response {protocol:string;status:number;server?:string;title?:string;contentType?:string;redirect?:string;available?:boolean;}
export function isWebEndpoint(endpoint:Response):boolean;
export function currentWebEndpoints<T extends Response>(device:{endpoints:T[]}):T[];
export function matchesFilter(device:{endpoints:Response[];status:string;kind:string;evidence?:string;openPorts?:number[]},filter:string):boolean;
export const FILTERS:string[];
export function loadFilter(storage:Pick<Storage,'getItem'>):string;
