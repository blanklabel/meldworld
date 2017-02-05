package main

import (
	"github.com/gorilla/websocket"
	"github.com/satori/go.uuid"
)

type Player struct {
	conn *websocket.Conn
	ID   string
	//name string
}

func NewPlayer(c *websocket.Conn) *Player {
	u := uuid.NewV4().String()
	p := Player{conn: c, ID: u}
	return &p
}
