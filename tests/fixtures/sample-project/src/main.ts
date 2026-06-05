import { UserService } from './user-service';
import { Logger } from './logger';

/**
 * Main entry point for the application.
 * Initializes the user service and processes requests.
 */
export function main(): void {
    const logger = new Logger('app');
    const service = new UserService(logger);
    service.getUser('user-1');
    service.createUser('Alice', 'alice@example.com');
}

/**
 * Handles incoming HTTP requests and delegates to the service layer.
 */
export function handleRequest(method: string, path: string): string {
    const logger = new Logger('http');
    logger.info(`${method} ${path}`);
    if (path === '/users') {
        const service = new UserService(logger);
        return JSON.stringify(service.listUsers());
    }
    return '404 Not Found';
}
