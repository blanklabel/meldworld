package main

import (
	"github.com/gorilla/websocket"
	"github.com/satori/go.uuid"
)

// Player construct to simplify dealing with that dude
type Player struct {
	conn *websocket.Conn
	ID   string
	//name string
}

// Just easy way of returning a UUID for a player
func NewPlayer(c *websocket.Conn) *Player {
	u := uuid.NewV4().String()
	p := Player{conn: c, ID: u}
	return &p
}
